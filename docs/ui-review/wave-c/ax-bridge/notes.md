# Wave C Desktop 原生 AX bridge 取证笔记

日期：2026-08-26。决策：[ADR-042](../../../adr/ADR-042-desktop-accessibility-bridge.md)。helper：[`scripts/ui-ax-dump.swift`](../../../../scripts/ui-ax-dump.swift)（`swiftc -O` 编译的进程外测试工具，不进入 Desktop 生产构建）。

## 1. 验证链路

1. `cargo build -p pawork-desktop --offline --features gpui/runtime_shaders --bin pawork-desktop` 构建包含 bridge 的 Desktop。
2. `swiftc -O -o /tmp/pawork-ui-ax-dump scripts/ui-ax-dump.swift` 编译 AX dump/action helper。
3. `target/debug/examples/ui_fixture seed --root /tmp/pawork-ui-ax.Erhmlr` 写入固定 fixture；随后以同一 example 的 `serve` 启动真 Host，并轮询 `host_ready` barrier。
4. 以隔离的 `PAWORK_DATA_DIR`、`PAWORK_UI_BARRIER_DIR` 和 fixture socket 启动真 `target/debug/pawork-desktop`；轮询 `timeline_stable`，不使用固定 sleep 作为状态就绪判据。
5. helper 按 Desktop PID 找到窗口，先以 `--press session-fx-ses-alpha-today --action-only` 选择会话，再等待新的 `timeline_stable`；随后以 `--set-value composer-input "AX bridge typed text" --action-only` 写入 Composer。
6. 再次导出完整 AX tree，并以 `screencapture -x -o -l 7189 window.png` 保存真窗口截图。
7. 对自己启动的 Desktop / Host 发送 SIGINT，执行 fixture `down` / `clean`，仅删除精确临时根 `/tmp/pawork-ui-ax.Erhmlr`。

## 2. 结果

- [`ax-tree.txt`](ax-tree.txt)：`nodes=75 truncated=0`；role 分布为 `AXApplication=1 AXButton=28 AXGroup=9 AXList=2 AXRadioButton=3 AXRow=24 AXStaticText=4 AXTabGroup=1 AXTextArea=2 AXWindow=1`，无 `role=?`。
- [`action-select-session.txt`](action-select-session.txt)：`AXPress` 目标 `session-fx-ses-alpha-today` 返回 `result=0`；最终树中该节点 `selected=1`，Timeline 已加载对应动态条目。
- [`action-set-value.txt`](action-set-value.txt)：`AXValue` 写入 `composer-input` 返回 `result=0`；最终树显示 `value="AX bridge typed text" focused=1 settable=[AXValue,AXFocused]`，Send 同步为 enabled 且暴露 `AXPress`。
- [`window.png`](window.png)：真 Pawork 三栏窗口；选中会话为“Refactor launcher tabs”，Timeline 已加载，Composer 可见写入文本，Send 为启用态，Inspector 停留 Terminal。
- disabled 控件保留正确 role / label / enabled 状态，但不发布 `AXPress`；纯 group / static text 不发布动作或可写属性。动作全部回到既有 `AppView` handler 与 enable gate。

## 3. 裁决与边界

**PASS（ADR-042 的 AX 补救与 Wave C 语义驱动源通过）。** 与失败基线 [`../ax-gate/`](../ax-gate/) 的 7 节点窗口壳相比，Desktop 现已从同一显式语义树导出稳定 identifier、role、label/value、状态、层级和 action；U2 可用 Swift helper 做真进程语义定位，不再依赖坐标点击。U3 的真窗口截图继续使用 `screencapture`，视觉差分复用 `scripts/ui-visual-diff.py`。

本证据不宣称 R1 已整体完成：State A 的完整 `reference/current/overlay/diff/mask/checklist` 闭环与故意漂移捕获仍属 Wave D；真实 IME、性能与全量 VoiceOver 验收仍留 R7/R10。bridge 当前仅实现 macOS，Timeline AX 面按可见/有界条目生成；Windows / Linux 平台实现不在 ADR-042 范围内。

## 4. 复审修复轮（2026-08-26，grok reviewer → 主代理修复）

复审发现 4 项 P1 / 2 项 P2，其中 4 项确认并修复、2 项带证据驳回；另自查发现 1 项回归并修复：

- **修复 · TaskRail 结构镜像**：`sidebar_ax` 原始终投影扁平会话列表，不随 grouping/collapse 变化。现与 `task_rail.rs` 同构：Timeline = 日期组头 → 项目头（`project-{bucket}:{key}`，桶限定防重复 id）+ 项目新建（`project-add-…`）+ 展开时会话行；Projects = 项目块。折叠项目只投影头部。
- **修复 · Inspector 触发器链路**：`inspector-toggle` 原直跳 `on_toggle_inspector`，跳过可见路径的 ActivityPopover。现折叠态 AXPress 走 `toggle_menu(Activity)` 并发布 `activity-popover` / `activity-open-changes`；`inspector-collapse` 保持收起。
- **修复 · IME composing 闸门**：AX Send 原直调 `send_current_message` 绕过 `is_composing()`。现与键盘 Enter（`on_send_message`）同一闸门。
- **修复 · 原生树原位刷新**：原 value/focus 变化即整树重建（流式文本每帧重建，外部 AX element 全失效）。现结构（identifier/role/press 能力/子树形状）不变时原位同步属性 + 只发 value/focus 通知；结构变化才重建。内部树同步一律 super 直调，不触发对外 action 回调。
- **修复（自查回归）· 构建期初始 value 被吞**：上一轮「gate 先于 super」修复把 `ElementState` 未安装时的写入整体早退，导致 build 期 `setAccessibilityValue:` 永远到不了 AppKit。本轮 build/refresh 统一 super 直调后恢复。
- **驳回 · Drop use-after-free**：`install` 对 NSView 显式 retain，Drop 恢复原 class 后才 release，view 不可能先于 bridge 释放；窗口关闭顺序不构成 UAF。
- **驳回 · Rc handler 线程 UB**：Apple 文档保证 AppKit 在主线程服务外部 AX 请求（与 GPUI UI 线程一致），`Rc` 不构成跨线程；另加 debug 构建主线程断言（`cfg(not(test))`）防回归。

**真窗口复验**（fixture seed → serve → desktop，screen unlocked）：

- 连接后 `session-list` 出现 `date-group-Earlier` → `project-Earlier_3afx-alpha-app`（value="3 tasks"）→ `project-add-…` → 3 个 `session-fx-ses-alpha-*` 行。
- `AXPress project-Earlier_3afx-alpha-app` → help 变 `Collapsed`，3 个 alpha 会话行消失、项目头与 `+` 保留（可再展开）；beta 组不受影响。
- `AXPress inspector-collapse` → `inspector-toggle` → 出现 `activity-popover` / `activity-open-changes`（value 为 Activity 摘要）→ press 后 Inspector 展开且 `inspector-tab-changes` selected。
- Changes 页签现位于 `changes-tabs` TabGroup 下（`changes-tab-files` selected=1）。
- `AXValue` 写入 `composer-input` 后值可见且属原位刷新路径；选择会话后 `send` 变 enabled=1。
- `terminal-back-to-bottom` 节点已随脱钩条件发布（本 helper 无滚轮动作，脱钩态未真窗口复验，与可见按钮同一 `is_following()` 条件）。
