# R6 Wave A — Inspector 层级与 Header Activity

> 状态：🟢 已收口（2026-08-29；render / AX / U1 与 Connected State A/B 结构断言全过；SSIM 分区记录值按 R3–R5 先例移交 R8 终局门禁，不追认为通过）

## 本波范围

- Inspector 顶层 Changes / Terminal / Resources 与 Changes 内 Files / Summary 形成明确两级 strip，默认落点改为 Changes。
- Inspector 折叠态 Activity 触发器自 StatusBar 迁至 Workspace Header 最右动作槽，Popover 从触发器下方向下展开并右缘对齐。
- render 与显式 AX 树共享 58/56px tab、2px 下划线、320px Activity 几何和可见性谓词。
- Changes 摘要只消费权威 diff 状态；无 Agent / Add tool capability 时不画假数据，Resources 保持诚实过渡面。

## 已实现

- 顶层 tab 固定 `100×58`、18px 字；二级 tab 固定 `96×56`、17px 字；选中态统一底部 2px accent 下划线。
- Header 最右 `40×37` 槽随 Inspector 状态互斥：展开态 New task，折叠态 Activity；StatusBar 只保留 F-13 居中运行状态。
- ActivityPopover 使用局部 `TopRight` 锚点，内容合同为 `320×320`；标题为 Activity，Changes 摘要点击后展开 Inspector 并定位 Changes。
- 固定宽 Panel 增加 `flex_none`，Workspace/main/right-column 增加 `min_w_0`。真窗口检查发现长空态文案曾把 440px Inspector 实际挤到约 114px，而 AX 仍报告 440；该根因已修复，并由长行 GPUI 壳层回归覆盖。

## 自动验证

- `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`：**132/132 PASS**。
- 覆盖：默认 Changes、两级 tab/Activity token、Header Activity 可见性矩阵及 render 锚点对应的 AX 精确 frame 公式、1440×1024 三栏及长内容下 Inspector 440px 不收缩、1080×720 折叠与 resize 恢复。
- `git diff --check`：PASS。
- 未运行全 workspace gate（当前任务合同不设全量门禁）。

GLM 只读审查未发现 P0/P1；唯一 P2 指出 Connected 真窗口取证受阻时 Header Activity 的 AX 锚点缺自动化钉子。已把触发器/Popover/内部摘要 frame 提取为单一几何函数并新增精确回归，随后重跑上述 132 项门禁通过。

## 真窗口证据（Connected State A/B，2026-08-29 收口）

早前「AX ready 阶段超时」阻塞的根因已定位：R5→R6 间 Desktop 侧改动不涉及 AX 链路（GPUI 0.2.2 mac 平台无任何 accessibility 代码，AX 树全靠自研 bridge `accessibility/macos.rs`）；实测递归态下进程完全健康（已 Connected、timeline_stable 已写、主线程空闲），递归自进程诞生即存在、激活无效、进程级持久，且按时段成簇出现——判定为 **macOS 26.6.2 对无 bundle debug 二进制的 AX server 注册间歇性故障**，非代码回归。更正此前记录：「GUI 停在 Connecting」为误判——stalled 证据里 barriers 目录为空只是 driver 在 ready 闸门前超时、从未执行到拷贝步；GUI 实际已进入 Connected 投影。

绕过落在 driver 层，均 fail-closed（失败安全，不误 PASS）：

- [ui-ax-dump.swift](../../../scripts/ui-ax-dump.swift)：检测递归签名（AXApplication≥2 且无 AXWindow 且无 identifier）后切换 `kAXWindowsAttribute`/`kAXMainWindowAttribute` 回退根 dump 并标注 `# WARN ax-fallback=axwindows`；健康路径输出逐字节不变。
- [ui-r6-wave-a-states.sh](../../../scripts/ui-r6-wave-a-states.sh) 与 [ui-wave-d-state-a.sh](../../../scripts/ui-wave-d-state-a.sh)：就绪轮询识别递归签名（含收口审查 P1 整改补上的「WARN 回退仍无 session-list」降级分支），触发 desktop-restart ≤3 次；仍递归则以 exit 3 + 证据收场。
- 结构断言新增三相位（[ui-wave-d-tools.py](../../../scripts/ui-wave-d-tools.py)）：`r6-state-a` / `r6-state-b-open` / `r6-state-b-resumed`，逐值对齐 theme.rs / app.rs / dropdown.rs 合同；脚本 unittest 16/16，同级全部 9 套件回归全绿。

通过证据（label `r6a-connected`，git_head=d793999，2026-08-29T05:51Z）：

- **State A**（选中 fx-ses-alpha-today 后）：`assert r6-state-a` PASS——默认 Changes、两级 strip 58/56px、tab 相邻性与选中态、header-new-task 40×37、长内容下 Inspector 440px 不收缩。[截图](connected/state-a/current.png) · [geometry](connected/state-a/geometry.txt) · [断言 JSON](connected/state-a/assert-r6-state-a.json)。
- **State B**：inspector-collapse → inspector-toggle 开 popover → `r6-state-b-open` PASS（320×320、右缘对齐、顶距 toggle 底 +4、heading 高 20、toggle 挂 header 子树、header-new-task 缺席）→ activity-open-changes 恢复 → `r6-state-b-resumed` PASS（toggle/popover 无残留、Inspector 回到 Changes、Files/Summary 选中态正确）。[geometry-open](connected/state-b/geometry-open.txt) · [断言 open](connected/state-b/assert-r6-state-b-open.json) · [断言 resumed](connected/state-b/assert-r6-state-b-resumed.json)。
- 全程 trace：[action-trace.txt](connected/action-trace.txt)，末行 `run done assert_a=0 assert_b_open=0 assert_b_resumed=0 gate_a=1 gate_b=1`；reference/current/overlay/diff 成套于 [connected/](connected/)。

视觉门禁：两状态分区 SSIM 均未达 0.99——State A：global 0.662、taskrail 0.694、header-left 0.940、header-right 0.883、timeline 0.679、composer-left 0.423、composer-right 0.620、inspector-body 0.614、inspector-right 0.800、statusbar 0.649；State B：global 0.618、taskrail 0.544、header-left 0.774、header-right 0.693、timeline 0.650、composer-left 0.449、composer-right 0.765、statusbar 0.462、popover-left 0.528、popover-right 0.573。主因仍是 fixture 演示内容与设计稿形状差，同 R3 拍板 c / R4 拍板 1 / R5 用户指令先例记为已知缺口移交 R8 终局门禁，不追认为通过。

早前三次阻塞样本归档于 [state-a-ax-stalled-r6a-1](state-a-ax-stalled-r6a-1/)、[-2](state-a-ax-stalled-r6a-2/)、[-3](state-a-ax-stalled-r6a-3/)，契约缺口一次归档于 [connected-attempt1](connected-attempt1/)；[State A 结构截图](manual-structure/state-a-unconnected.png)（rail 288 / workspace 712 / Inspector 440、两级 strip 可见）仍只作结构证据。

## 遗留与移交

- 分区 SSIM ≥0.99 与 fixture 演示数据重塑：移交 R8 终局视觉门禁（条款见 [plan/R7-R8-ui-quality-gates.md](../../../plan/R7-R8-ui-quality-gates.md) §3）。
- AX server 注册 flake 的产品侧复核（bundled/签名形态）：登记 ROADMAP §5，R7 VoiceOver/AX 门禁前复核；driver 层绕过已就位。
- 断言覆盖已知子集缺口（收口审查 P2 登记）：popover 内部偏移（heading/summary offset、左右 inset 20/宽 280）与 `.max(header_frame.x)` 钳制未断言，本次采集值恰好全部吻合；需要时随 Wave B 补强。
