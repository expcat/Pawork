# R2 Wave B 收口记录（2026-08-27）

> 范围：窗口状态继承壳层（启动/连接中/无 Host/空 task/失焦）、StatusBar/分隔线细节校准、window_min_size 拍板落地、U2 模拟操作 driver 扩展（启动/focus/blur/resize/Inspector 开合）与 State B shell 证据。TaskRail 功能扩张（F-03/F-04）按 ROADMAP 指针不进入本波。
> 驱动：新增 [scripts/ui-wave-b-states.sh](../../../scripts/ui-wave-b-states.sh)（复用 R1 Wave D 链路：barrier/轮询同步、ui-fixture 隔离实例、AX 语义定位）。

## 1. 实现落点

- **空态引导**（gui-design 空态原则「无会话时主区只有一句提示和 Composer」）：[timeline.rs](../../../apps/desktop/src/ui/timeline.rs) 在无 active session 且条目数（含审批卡）为 0 时居中渲染一句 tertiary 引导「Select a task from the rail, or press ⌘N to start a new one.」；可见条件收敛为 projection 谓词 workspace_empty_hint_visible()，Disconnected 保留旧条目时不显示。AX 树同批补只读 workspace-empty-hint StaticText 节点（无 action，[accessibility/app.rs](../../../apps/desktop/src/ui/accessibility/app.rs)），文案经 WORKSPACE_EMPTY_HINT 常量与视觉同源。Connecting / Connect failed / 无 Host 三态走同一路径（均无 active session），壳层三栏 + StatusBar 原样继承。
- **Reconnect 相位**：rail 的 Reconnect 主按钮改为仅 Disconnected / ConnectFailed 显示（projection.show_reconnect()）；Connecting 属进行中，不再同时出现「Connecting…」徽标与重复重连入口。
- **Terminal 占位去调试口吻**：无输出时显示「Terminal output will appear here.」（删去「No local PTY — host streams TerminalOutput.」开发者说明）。
- **StatusBar F-13 校准**：[status_bar.rs](../../../apps/desktop/src/ui/components/status_bar.rs) 新增 centered()——信息串在行内绝对居中、不随右侧触发器宽度偏移（对照定稿图信息居中）；run_status_label 改定稿语序与竖线分隔「Task — tokens | Quota unavailable | — tok/s | Run {idle|mm:ss|—}」，缺权威来源仍一律 — / unavailable，不伪造数值。Inspector trigger 保留最右（F-12 迁移到 Workspace Header 后再撤）。24px 高度、顶描边、rail/Inspector 分隔线贯通性经审查确认已合规（shell_layout 既有 gpui 测试钉住），无双线无断线，本波不改几何。
- **window_min_size 拍板（主代理决定）**：钉 **1080×720**（[main.rs](../../../apps/desktop/src/main.rs) WINDOW_MIN_SIZE 注入 WindowOptions.window_min_size，gpui 0.2.2 macOS 落地 setMinSize）。依据：design/README §2 的 1080×720 是响应式功能门禁底线——再窄 Workspace 击穿 ≥560 合同（1080 带宽下 240+560+折叠 Inspector 已满）；Wave A 遗留的「<800px 可低于 560」缺口由窗口系统直接封闭。

## 2. 定向回归（全绿）

cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders：**78 passed / 0 failed**（基线 74 + 新增 4）。新增：

- projection::tests::run_status_label_uses_final_order_and_vertical_separators（idle / 缺起始时长 / 运行中三态，诚实占位）
- projection::tests::reconnect_shows_only_for_disconnected_or_failed（四相位）
- projection::tests::workspace_empty_hint_requires_no_session_and_no_entries（默认可见 / 有 session / Disconnected 保留条目）
- main::tests::window_min_size_pins_design_responsive_floor（1080×720 常量钉住）

脚本侧：python -m unittest test_ui_wave_d_tools test_ui_wave_b_tools **25/25 绿**（wave-b 17 + wave-d 8：相位断言 / shell-manifest / driver 守卫 / resize 合同标记 / 审查补强的负向断言与 normalize 护栏）；bash -n × 4；swiftc -O 编译 ui-ax-frames.swift 通过。

## 2.1 审查修复（glm_reviewer 一轮，同批落地）

- **P1 AX 镜像漂移**：accessibility/app.rs 的 Reconnect 节点发布条件由旧的 !connected 改为与视觉同源的 projection.show_reconnect()（Connecting 相位不再发布可 press 的幽灵节点，ADR-042「触发器语义与可见路径一致」）。
- **P2 负向断言补强**：empty 相位新增 reconnect-absent；collapsed/resumed 相位新增 workspace-empty-hint-absent（防谓词回归恒真）；narrow/collapsed 相位统一新增 inspector-toggle-present（F-12 迁移前的临时主路径）。
- **P2 Terminal 占位同源**：占位文案收敛为共享常量 TERMINAL_EMPTY_OUTPUT，视觉与 AX 树同值。
- **P3 normalize 护栏**：cmd_normalize 显式拒绝非 1440×1024 root frame（防 1080 相位截图被静默放大误用），返回 3 并说明。

## 2.2 收口审查（主代理 + glm_reviewer 复核）

- 删除 `sidebar_ax` 在改为 `show_reconnect()` 后遗留的未使用 `connected` 局部变量（本波引入的 dead_code 警告）。
- 复跑 cargo 78/78、脚本 unittest 25/25 全绿；基线 3 条既有 dead_code 警告仍在（`replaces_baseline` / `resume_outcome` / `tool_completed`），非本波引入。
- grok_reviewer 二次只读审查超时无增量输出，已中断；不以锁屏环境下的 U2 真窗口结果作为通过条件。

## 3. U2 driver 扩展（scripts/）

- 新增 [scripts/ui-wave-b-states.sh](../../../scripts/ui-wave-b-states.sh)：编排 启动空态捕获（workspace-empty-hint 断言 + 截图/归一）→ focus/blur 截图 → resize 1440→1080（narrow 断言：rail 240、Inspector 缺席）→ 回 1440（restored 断言）→ AXPress 开会话 → inspector-collapse/inspector-toggle 开合 → State B 捕获（collapsed 断言：Inspector 列缺席 + inspector-toggle + activity-popover 在场 + 截图/归一）→ desktop-restart 重开验证（AXPress 重开会话、resumed 断言：host 侧会话/时间线可恢复）。同步只用 barrier/轮询；相位断言本身充当布局收敛轮询。
- 新增 [scripts/ui-ax-frames.swift](../../../scripts/ui-ax-frames.swift)（独立 helper，--resize WxH 收敛轮询）、[scripts/ui-focus-switch.sh](../../../scripts/ui-focus-switch.sh)（System Events activate/deactivate，激活态轮询确认）、[scripts/test_ui_wave_b_tools.py](../../../scripts/test_ui_wave_b_tools.py)。
- [scripts/ui-fixture.sh](../../../scripts/ui-fixture.sh) 新增 desktop-restart（只停/起 desktop，host/数据/barrier 保留）；[scripts/ui-wave-d-tools.py](../../../scripts/ui-wave-d-tools.py) 加相位感知断言（empty/narrow/collapsed/restored/resumed）与 shell-manifest，默认 initial/final 行为不变（Wave D 回归由同一测试箱钉住）。
- **主代理收口修正**：重开相原实现等待「新进程自动恢复原 active session」——但 apply_fresh_snapshot 不恢复进程内 active_session_id（desktop.md §4.1），必超时。已改为重开后等新 barrier 基线再 AXPress 重开会话（host 持久化 + 重连的诚实 resume 语义）。
- **State B 只产 shell 证据**：不做 zones/current 映射与 SSIM——F-05（Workspace Header）/F-12（Popover 迁右上）未落地，当前 Popover 仍由 StatusBar 触发，视觉门禁留待后续波次如实记录。

## 4. U2 真窗口门禁：🟢 通过（2026-08-27 14:01 +08）

- 首轮（11:09 +08）因 macOS 自动锁屏未完成：IOConsoleLocked=true 下 CGWindowList 不报告应用窗口、AX 只剩 AXApplication 空链、screencapture 无法取图——与 R1 Wave D 记录的环境风险一致，非代码回归。
- 屏幕解锁后重跑通过：`scripts/ui-wave-b-states.sh run --out docs/ui-review/r2-wave-b/u2 --label wave-b-1`（git_head=b9f79ec，exit 0）。五相位断言全绿——empty（workspace-empty-hint 在场 + reconnect 缺席 + 时间线空）→ focus/blur（激活/失焦截图）→ narrow（1080：rail 240、Inspector 列缺席、inspector-toggle 在场）→ restored（回 1440 三栏复原）→ collapsed（State B：inspector-collapse / inspector-toggle AXPress、Inspector 列缺席 + activity-popover 在场 + 归一截图）→ resumed（desktop-restart 后 AXPress 重开会话，timeline 25 条恢复，seq 1→2）。run-manifest.json structural_pass=true。
- 证据归档 [u2/](u2/)：empty-state.png / focus-active.png / focus-blurred.png / narrow-1080.png / state-b.png（归一 1440×1024）、五相位 assert-*.json 与 ax-tree-*.txt、action-*.txt、geometry-*.txt、barriers 与日志。
- 已知非阻塞项：composer-height 各相位均为 156（合同 88–94），即 Wave D 已登记的 F-09 视觉漂移，断言标记 blocking=false，不属本波回归。

## 5. 遗留与候选

- Inspector 默认 Terminal 页在空态的存在感（UI_Review Step 0「抢占首屏注意力」）本波只去调试文案；默认页签/默认开合决策归 R6（Inspector 属主），不在此拍板。
- F-13 完整项（窄窗 status details popover、tokens/quota/tok/s 权威数据源）随 R5/R6 数据面推进；本波只落地定稿语序、竖线分隔与居中布局。
- State B zones current 映射与分区 SSIM 待 F-05/F-12 落地后补齐。
