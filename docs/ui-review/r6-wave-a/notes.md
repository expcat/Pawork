# R6 Wave A — Inspector 层级与 Header Activity

> 状态：🔵 进行中（render / AX / U1 已实现并通过；State A 结构截图已人工核对；同 fixture 的 Connected State A/B 自动证据仍待收口）

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

## 真窗口证据与当前阻塞

自动 State A driver 连续两次在 AX ready 阶段超时。两次均满足 `ax_trusted=true`、窗口存在、fixture host 接受连接，但外部 AX 查询只得到递归 `AXApplication → AXApplication`，没有任何 identifier：

- [第一次 trace](state-a-ax-stalled-r6a-1/action-trace.txt) / [AX timeout](state-a-ax-stalled-r6a-1/ax-tree-probe-timeout.txt)
- [第二次 trace](state-a-ax-stalled-r6a-2/action-trace.txt) / [AX timeout](state-a-ax-stalled-r6a-2/ax-tree-probe-timeout.txt)

手工以前台 Desktop + 同一 seeded fixture host 复核时，独立 `--probe` 连接成功并报告 `sessions=7, models=9`，证明 socket/fixture 数据可用；但 GUI 进程停在 Connecting，未形成 Connected 投影。保留的 [State A 结构截图](manual-structure/state-a-unconnected.png) 因此只作为结构证据，不作为同 fixture Connected 或视觉门禁通过证据。截图含窗口阴影；内容区横向边界实测为 rail 288px、workspace 712px、Inspector 440px，且可见默认 Changes、58px 顶层 strip 与 56px Files/Summary strip。对应外部 AX 递归样本见 [ax-recursion.txt](manual-structure/ax-recursion.txt)。

因此本波目前只可标记为“已实现 + U1 通过”。State B 折叠态 Activity 真窗口开合/锚点、Connected State A 数据投影、reference/current/overlay 与区域指标均未取得，不得追记为通过。

## 下一步

1. 恢复自动启动路径的 macOS AX bridge 可发现性，并确认 GUI 进程进入 Connected 投影。
2. 用同一 fixture 重采 State A 与 State B，验证 Header Activity 开合、右缘对齐、约 320px 高度、摘要点击回到 Changes。
3. 生成 reference/current/overlay/diff 与结构断言；全部通过后才关闭 Wave A，随后进入 R6 Wave B。
