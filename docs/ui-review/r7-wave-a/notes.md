# R7 Wave A — 组件状态矩阵与 AX 基线

> 状态：🟢 已关闭（2026-08-29 开启；2026-08-30 用户确认九图通过，并批准原生 AX / 键盘 / U2 替代本波 VoiceOver）

## 开启决策

- 2026-08-29 用户曾明确指令跳过当时未收口的 R6、直接进入 R7；R6 随后恢复并完成 Wave B。2026-08-30 用户确认将 R6 State A/B Inspector/Activity 分区 SSIM `≥0.99` 移交 R8，R6 正式退出，本波恢复人工验收；移交项仍不得记为通过。
- 本波只处理跨组件交互状态与 Accessibility 基线，不扩 GUI wire，不改 Host / Policy / storage，不新增依赖，也不消费真实 Provider 凭证。
- Desktop 仍是独立 GPUI 进程，业务依赖只允许 `pawork-client`；`gpui = 0.2.2` 与 ADR-042 原生 AppKit AX bridge 决策保持冻结。

## 固定输入与事实基线

- Reference：[`state-a/reference.png`](../state-a/reference.png)（State A，1440×1024）。
- 已归档 current 线索：[`r6-wave-a/connected/state-a/current.png`](../r6-wave-a/connected/state-a/current.png)。它早于当前未提交工作树，只用于定位；Wave A 改实现前必须以当前源码和同一 fixture 重采一份 fresh current / AX tree / run manifest，禁止把旧图当本波通过证据。
- 组件清单起点：[`component-manifest.md`](../component-manifest.md)。其几何与 capability 枚举仍可用，但部分“现实现差异”停留在 R1，不能直接当当前事实；Wave A 先对照源码与真实 AX 树重建可执行矩阵。
- 已知平台风险：macOS 26.6.2 对无 bundle debug 二进制的 AX server 注册会间歇返回递归 `AXApplication`。既有 AXWindows 回退与 Desktop restart≤3 只负责 fail-closed 取证，不算根治；本波优先比较 bundled/签名形态。
- 当前工作树 Desktop 定向基线：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 为 **144 passed / 0 failed**。这只证明当前代码测试基线，不是 R7 Wave A 验收。

## 本波写入集

- 生产：`apps/desktop/src/ui/` 内组件状态、focus/keyboard 与 Accessibility 同源接线；仅在矩阵发现确定缺口后修改。
- 测试：`apps/desktop` 定向测试与 `scripts/` 下 R7 AX/交互 driver。
- 文档：本目录、[`component-manifest.md`](../component-manifest.md)、[`desktop.md`](../../spec/crates/desktop.md)、必要时 [`gui-design.md`](../../gui-design.md) 与 [`design/README.md`](../../../design/README.md)、本任务书和 ROADMAP。
- 禁止范围：`crates/protocol`、`crates/app` Host 行为、Policy、安全审批、fixture 业务数据、设计 reference 变更。

## 执行切片

1. **A1 · current 与清单重建**：重采 State A fresh current/AX tree；逐项核对稳定 identifier、role/name/value/enabled/focused/selected/bounds/action，以及 default/hover/active/focus/disabled/loading/error/selected。
2. **A2 · 三路径差额**：把每个可达 action 映射到 mouse、纯键盘、AX；缺一即列为 gap，修复必须回到同一 AppView handler 与 enable gate。
3. **A3 · AX 注册复核**：对同一源码比较 debug 裸二进制与 bundled/签名形态；保留首次失败、重试和环境信息。递归树、仅 Window/traffic lights 或缺 Pawork identifier 均直接失败。
4. **A4 · 收口证据**：运行 U0/U1、真窗口 U2 与 State A hover/active/focus 补充图；归档 action trace、focus trace、AX tree、截图和 manifest。人工 overlay 未签字时只记“等待人工验收”。

## Wave A 退出条件

- 当前可见 manifest 每项都有状态集合与 mouse/keyboard/AX 覆盖结论；不可用/隐藏项给出权威 capability 原因。
- AX tree 含 Pawork 自定义节点，焦点与 enabled/action 同源；至少一条主路径由 mouse、键盘和 AX 达到同一可观察终态。
- State A 的 hover/active/focus 补充证据成套；自动门禁与人工 overlay 分开记录。
- bundled/签名复核给出可重复结论；平台限制若仍存在，保留 fail-closed 失败包并明确阻塞范围。

## 已执行（滚动记录）

### 2026-08-29 白班

- **A1 fresh 基线**：[baseline-debug/](baseline-debug/)（09:53–09:54 UTC）——State A/B fresh current.png、ax-tree（85 nodes / pass:True assert）、geometry、run-manifest。视觉 diff 全区 FAIL（SSIM 0.42–0.94）按 R3–R6 先例移交 R8，不阻塞本波。
- **A2 三路径差额与修复**：
  - 首跑取证 [u2-three-path-dual-focus-gap/](u2-three-path-dual-focus-gap/)：菜单打开时触发器与高亮项双 focused（assert-button-enter-grouping-menu focus check FAIL）。
  - 修复：触发器 focused 改为 open_menu.is_none() && focus.is_focused(window)（grouping/scope/add-task/reconnect，accessibility/app.rs:396/416/445/472）；workspace-confirm 浮层纳入高亮体系（menu_item_count + focused(ix==highlight)）；TL-04 Review changes / TL-07 EntryMenu / TR-06 Reconnect 键盘 on_activate 接线收口。
  - driver 断言同步：ui-r3-wave-b-tools.py button-enter 相位改为「浮层打开 + AX 焦点唯一在高亮项」口径；python 单测 15/15（含 dual-focus 负向钉）。
  - 桌面定向基线：cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders = 142 passed / 0 failed。
  - 复跑 [u2-three-path-fixed/](u2-three-path-fixed/)（及 attempt1/2）：前段相位全绿，卡 rail-focus-add-task——该相位旧断言（点击 add-task 后焦点在触发器）与新焦点口径冲突，属断言待同步而非实现缺口（矩阵 §4 登记）。
- **A1/A2 主交付**：[component-matrix.md](component-matrix.md)——45 组件 × 状态集 × mouse/keyboard/AX 覆盖结论 × manifest 过期行修订（F-05 Header 已实现、TL-04 Review changes 已实现、SB-02/D-01 触发器迁移已落地等）。

### 2026-08-29 晚班（续）

- **U2 全量复跑收口（通过）**：[u2-three-path-fixed/](u2-three-path-fixed/) 第三轮全相位绿——Tab 链、裸 Enter 开 grouping/workspace-confirm/scope 菜单且 AX 焦点唯一在高亮项、菜单键盘选择、workspace cycling、断线重连、blocked/unread 持久可见（attempt1/2 为修复前失败前史）。
- **断言口径同步**：ui-r3-wave-b-tools.py 的 rail-focus-add-task 相位改为「workspace-confirm 存在 + 唯一焦点在高亮项」（R7 单焦点合同）；test_ui_r3_wave_b_tools.py 补正/负向用例，15/15。
- **AX 发布缺陷修复**：accessibility/app.rs 的 ProjectAdd 节点误链两个 `.focused()`（第二个 reconnect_focus 覆盖第一个），删除多余行。
- **MAIN_PATH_TAB_STOP_IDS 同步**（ui/mod.rs）：删除死条目 run-summary-review-changes，补 header-new-task/reconnect/timeline-back-to-bottom（九项 + 动态行级 FocusHandle）；desktop.md 焦点档位段落同步。
- **A3 bundled/签名对照（通过）**：[ax-forms/](ax-forms/)——raw 与 bundled-signed 首启 AX 树均分类为 pawork-identifiers（identifier_count 46/63，required 五项全中，无 AXWindows 回退）；run-manifest 含同 payload SHA-256、codesign 验证、sw_vers 26.6.2、git status。
- **states 补充采集（平台阻塞，fail-closed 归档）**：20:23 起平台级 AX 递归劣化持续——窗口存在但 AX 树为 26 层 AXApplication 递归链、无 session-list、仅系统菜单 identifiers（[state-supplement-attempt7/](state-supplement-attempt7/) 超时取证；attempt8/9/10 各经 3 次 desktop-restart 均复现后按设计 fail-closed，终态取证见各目录 ax-tree-probe-recursive-final.txt）。与当日 09:53–11:11 baseline/ax-forms 及 ~19:11 u2 成功时段对照，判定为平台时段性劣化而非 app 形态问题。driver 同步加固：is_ax_recursion 第三判定（AXApplication 行 ≥5 且无 session-list → 病态）与 place-main 看门狗（30s kill）+ 有界重试（≤3）。

### 2026-08-29 晚班收口

- **A4 State A hover/active/focus 补充图（自动门禁通过）**：[state-supplement/](state-supplement/)（22:46 CST，bundled-adhoc 签名启动）九张成套：hover（grouping / session 行 / model-picker / inspector-tab-terminal / send）+ active（grouping 按住与菜单打开）+ focus（session 行 selected、composer-input focused）。Driver 修复：place-main 看门狗 wait 不得被 set -e 打断；AX dump 带超时；Escape 后重新 activate；session 行点击后焦点按产品合同落到 composer（断言 selected=1，不要求行级 focused=1）。attempt7–10 仍保留为 AX 递归劣化 fail-closed 包。
- U0/U1：cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders 为 142 passed / 0 failed；python3 scripts/test_ui_r3_wave_b_tools.py 为 15/15。

### 2026-08-30 overlay 续查

- **发现与修复**：以五张同状态 hover 截图的逐像素 RGB 中位数为基线核对时，原 [`shot-hover-inspector-tab-terminal.png`](state-supplement/shot-hover-inspector-tab-terminal.png) 相对基线变化 **0 pixels**，未证明可见 hover；源码只把 `text.secondary` 改为 `text.primary`，也不符合 [`design/README.md` §8.1](../../../design/README.md) 的「hover / active 只改背景」合同。`ui/inspector.rs` 已收敛为 `surface.raised` 背景，active 复用同色，不改页签几何。
- **定向验证**：Desktop 定向门禁 **144 passed / 0 failed**；当前源码以 bundled-adhoc 真窗口重采到 [`state-supplement-hoverfix/`](state-supplement-hoverfix/)。Terminal hover 图相对同轮五图 hover 中位基线变化 **5,577 pixels**，边界 `(1117, 0)–(1216, 56)`，精确落在 100×58 页签内；机器可读结果见 [`pixel-check.json`](state-supplement-hoverfix/pixel-check.json)。
- **证据口径**：本次只替换受影响的 Terminal hover 图；其余八图仍以 [`state-supplement/`](state-supplement/) 为准。重复全套采集的第一张 grouping hover 因指针起始位置与目标重合而未再次触发 hover，不纳入新基准，也不把重跑当作通过依据。
- **人工边界**：用户明确要求不使用 VoiceOver；本轮保持关闭且未执行该走查。该要求本身不自动等于豁免验收标准，因此在取得下述替代决定前，Wave A 仍保持开启。

## 2026-08-30 用户验收与关闭决定

- 用户确认 State A hover / active / focus 九图通过：八张以 [`state-supplement/`](state-supplement/) 为准，Inspector Terminal hover 以修复后 [`state-supplement-hoverfix/shot-hover-inspector-tab-terminal.png`](state-supplement-hoverfix/shot-hover-inspector-tab-terminal.png) 为准。
- 用户批准本波以原生 AX tree/action + 纯键盘 + U2 替代 VoiceOver：对应证据为 [`component-matrix.md`](component-matrix.md)、[`u2-three-path-fixed/`](u2-three-path-fixed/) 与 [`ax-forms/`](ax-forms/)。
- 边界：VoiceOver 始终未执行、不记为通过；上述替代只关闭 R7 Wave A，不证明屏幕朗读措辞 / 顺序，也不静默豁免 R8 的系统级验收。
- 结论：自动门禁与人工九图均通过，平台递归 AX 失败包已 fail-closed 保留，Wave A 关闭并进入 Wave B。
