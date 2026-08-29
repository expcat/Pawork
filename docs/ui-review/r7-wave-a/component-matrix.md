# R7 Wave A — 组件状态矩阵（mouse / keyboard / AX 三路径基线）

> 状态：🔵 自动门禁通过、等待人工 overlay（2026-08-29）<br>
> 固定视觉输入：State A（[../state-a/reference.png](../state-a/reference.png)，1440×1024）＋当前源码 fresh AX 树（[baseline-debug/state-a/ax-tree.txt](baseline-debug/state-a/ax-tree.txt)，85 nodes）<br>
> 组件枚举起点：[../component-manifest.md](../component-manifest.md)（45 条）；本文只回写与现实现不符的行，capability 判定沿用 manifest §7 口径（real / partial / honest-hidden / unavailable / 结构缺口）

## 1. 口径

- **三路径**：mouse = 真实点击（CGEvent click / GPUI on_click）；keyboard = 纯键盘（Tab 链 + 行级/根级 Enter/Space/方向键 + 全局 keymap）；AX = AppKit bridge（ADR-042）AXPress/AXFocus/AXSetValue。三路径必须收敛到同一 AppView handler 与同一 enable gate，路径分离即 gap。
- **状态集**：default / hover / active / focus / disabled / loading / error / selected，以 manifest §6 为目标；AX 侧以 role/name/value/enabled/focused/selected/actions 观测。
- **证据指针**：r3b = [r3-wave-b/u2-nav](../r3-wave-b/u2-nav/)（Tab 链、行激活、菜单键盘、cmd-alt、断线重连）；r6a = [r6-wave-a](../r6-wave-a/)（Activity 迁移、connected 快照、AX stall 取证）；r6b = [r6-wave-b/u2-final-4](../r6-wave-b/u2-final-4/)（Changes/Terminal 键盘走查、T1/C1/C3/R1 相位）；r7 = 本目录 [u2-three-path-fixed](u2-three-path-fixed/)（裸 Enter 开菜单/浮层、双焦点修复后全量复跑）；u1 = 桌面单测（apps/desktop u1_probe/mod）。源码指针行号以当前工作树为准。
- **✓ 判定**：既有 driver 相位、桌面单测或源码同源接线三者其一为证，且未发现反例；**gap** 列出缺口与证据；**—** = 无动作语义（纯展示/文本）。

## 2. 矩阵

### 窗口与 TaskRail（TR-01…TR-12）

| ID | AX identifier（现实现） | 状态集 | mouse | keyboard | AX | manifest 修订 |
| --- | --- | --- | --- | --- | --- | --- |
| WIN-01 | pawork-root（AXApplication） | 常驻；traffic-light 安全区 | — | AppKit Tab monitor（r3b） | 树根 | F-01 白 titlebar 维持结构缺口 |
| TR-01 | task-rail-grouping（Button，value=当前分组） | default/hover/focus | ✓ on_click（r3b grouping-menu-open-a） | ✓ Tab 档 -19 + 裸 Enter on_activate（r7 button-enter-grouping-menu） | ✓ AXPress | 本波修订：菜单打开时触发器让出 focused，AX 焦点移交高亮项（双焦点修复） |
| TR-02 | grouping-menu + group-timeline / group-projects（selected=当前项） | open/closed；行 hover；选中 checkmark | ✓ MenuRow click | ✓ 菜单打开即接管 ↑/↓/Enter/Escape（r3b grouping-*-keyboard） | ✓ AXPress → on_select_grouping | — |
| TR-03 | project-scope（Button，value=scope 标签） | default/hover/focus | ✓ | ✓ Tab 档 -20 + Enter（r7 button-enter-scope-menu） | ✓ | 同 TR-01 焦点口径 |
| TR-04 | scope-menu + scope-all / scope-&lt;id&gt; 动态 | open/closed；行 hover | ✓ | ✓ ↑/↓/Enter（r3b scope 相位） | ✓ | — |
| TR-05 | connection-status（StaticText，value 四态文字） | Connected/Connecting/Disconnected/ConnectFailed | — | — | — | — |
| TR-06 | reconnect（Button，仅断线态发布） | hidden(常态)/default/hover/focus | ✓ | ✓ track_focus + on_activate + Tab 档 -17（R6B 接线，本波补全 reconnect_focus 字段与 AX focused） | ✓ AXPress（r3b action-press-reconnect） | — |
| TR-07 | add-task（Button，help=禁用原因） | default/hover/focus/disabled | ✓ | ✓ Tab 档 -18 + Enter（r7 button-enter-add-task-popover） | ✓ | 本波修订：workspace-confirm 打开时焦点移交首项（同菜单口径） |
| TR-08 | date-group-&lt;label&gt;（Group） | 空桶不渲染 | — | — | — | — |
| TR-09 | project-&lt;bucket&gt;_&lt;key&gt; 头 + project-add-&lt;bucket&gt;_&lt;key&gt; 定向＋ | 展开/折叠；hover；断线禁用 | ✓ 折叠/建稿（r3b rail-focus-alpha-header、action-press-project-add） | ✓ 行链 ↓/→/↑ + ListRow on_activate | ✓ AXPress 动态 arm | — |
| TR-10 | session-&lt;id&gt;（AXRow，help=状态词，selected） | selected/hover/focus/folded/filtered | ✓ 行 click 打开（r3b） | ✓ Enter 行级激活（r3b key-open-task）+ cmd-alt ↑/↓/n | ✓ AXPress（r6b action-select-alpha） | — |
| TR-11 | 同 TR-10 行形态（缺 workspace_id 桶） | 同 TR-10 | ✓ 源码同源（rail_project_entries None 分桶） | ✓ 同 TR-10 行链 | ✓ 同构 arm | 真窗口专属相位缺失：fixture 无 Unassigned 样本行，结论依据源码同源（gap-evidence） |
| TR-12 | （无节点） | honest-hidden | — | — | — | 无账户 capability 维持 |

### Workspace Header 与 Timeline（WS-01、TL-01…TL-08）

| ID | AX identifier | 状态集 | mouse | keyboard | AX | manifest 修订 |
| --- | --- | --- | --- | --- | --- | --- |
| WS-01 | workspace-header + header-title / header-branch / header-status（条件）/ header-new-task | 常驻；live 终态文字条件发布 | ✓ | header-new-task ✓ Enter on_activate（mod.rs:675） | ✓ AXPress | **F-05 已过期**：Header 整块已实现（R6A），「现实现整块缺失」行作废；branch 终态随 diff 响应显示 |
| TL-01 | timeline-entry-evt-&lt;id&gt;-&lt;seq&gt;（AXRow，静态内容） | 常驻 | — | — | — | — |
| TL-02 | 同 TL-01（增量合并） | 常驻/流式增量 | — | — | — | F-07 视觉层级维持 |
| TL-03 | tool-group-*（Group）+ tool-row-*（行） | 每行 pending/running/succeeded/failed/cancelled 文字+图标 | — | — | — | F-08 可折叠组/时长维持视觉目标 |
| TL-04 | run-summary-* 卡 + run-review-changes-*（Button） | 终态条件渲染；按钮 hover | ✓ on_review_changes | ✓ Enter on_activate（timeline_entry.rs:551，本波接通） | ✓ AXPress（动态 arm） | **已过期**：manifest「不渲染按钮」行作废——Review changes 已实现（联动 Inspector Changes） |
| TL-05 | run-footer-*（文本） | 无终态不显示 | — | — | — | — |
| TL-06 | approval-card + approve-once / approve-for-run / approve-deny | pending 条件渲染；断线禁用 fail-closed | ✓ | ✓ cmd-1/2/3 + on_activate（approval_card.rs:104） | ✓ AXPress | r6b t2-denied 相位 |
| TL-07 | entry-menu-*（Button）→ fork-&lt;id&gt; 菜单行 | 「···」点击开；非闭合 run 边界禁用 | ✓ | ✓ Enter on_activate（timeline_entry.rs:235，本波接通）+ 菜单 Enter 选择 | ✓ 动态 arm | — |
| TL-08 | timeline-back-to-bottom（Button，脱钩才浮出） | hidden(贴底)/visible | ✓ | ✓ on_activate（timeline.rs:218） | ✓ AXPress | — |

### Composer（CP-01…CP-11）

| ID | AX identifier | 状态集 | mouse | keyboard | AX | manifest 修订 |
| --- | --- | --- | --- | --- | --- | --- |
| CP-01 | composer（Group） | 常驻 | — | — | — | F-09 几何维持 |
| CP-02 | composer-input（AXTextArea，settable AXValue/AXFocused） | focus/IME marked range/多行 | ✓ click focus（u1 mouse_click_focuses_text_input） | ✓ Tab 链尾 + 全套编辑键 + Enter 发送裁决（u1 keystrokes/shift-enter） | ✓ AXFocus + AXSetValue（r3b action-set-value-fixture-fail） | — |
| CP-03 | model-picker（Button，value=effective model，enabled=can_switch_model） | default/hover/focus/disabled | ✓ | ✓ on_activate（input_area.rs:53） | ✓ AXPress | — |
| CP-04 | model-menu + model-&lt;provider&gt;:&lt;id&gt; 动态 | open/closed；翻假自动关 | ✓ | ✓ ↑/↓/Enter | ✓ | — |
| CP-05 | （无节点） | honest-hidden | — | — | — | 维持 |
| CP-06 | （静态标签，无独立节点） | 常驻 | — | — | — | partial 维持（无 picker 命令） |
| CP-07 | （无独立节点；unavailable 文案） | 常驻 | — | — | — | partial 维持（分子无 wire 来源） |
| CP-08 | send（Button，enabled=can_send） | idle 显示；disabled | ✓ | ✓ on_activate（input_area.rs:93） | ✓ AXPress（r3b action-press-send） | — |
| CP-09 | cancel（同槽 composer_action_focus） | run 中条件 | ✓ | ✓ on_activate + cmd-. | ✓ | — |
| CP-10 | status hint 文案（composer 区文本） | 轮换/禁用原因 | — | — | — | — |
| CP-11 | workspace-confirm + workspace-confirm-&lt;ws-id&gt; | 条件打开；行 hover | ✓ MenuRow click | ✓ Enter 打开 + ↑/↓/Enter 选择（menu_item_count 已含 WorkspaceConfirm） | ✓ AXPress | 本波修订：高亮项接 AX focused（focused(ix == highlight)），触发器让位同菜单口径 |

### Inspector 与 StatusBar（IN-01…IN-09、SB-01、SB-02）

| ID | AX identifier | 状态集 | mouse | keyboard | AX | manifest 修订 |
| --- | --- | --- | --- | --- | --- | --- |
| IN-01 | inspector-tabs（TabGroup）+ inspector-tab-changes / terminal / resources | 当前页 selected+下划线 | ✓ click | ✓ ←/→/Enter/Space 根级 tab-list 分派（mod.rs:1560 区） | ✓ AXPress | Add tool honest-hidden 维持（D-02） |
| IN-02 | changes-tabs + changes-tab-files / summary + changes-refresh | 当前项 selected | ✓ | ✓ ←/→/Enter/Space（mod.rs:1585 区） | ✓ | — |
| IN-03 | 汇总行文本 | — | — | — | — | — |
| IN-04 | changes-file-list + changes-file-&lt;path&gt; | 选中高亮 | ✓ | ✓ ↑/↓/Enter 根级行链（r6b c3-file-focus） | ✓ | — |
| IN-05 | DiffView 内容（AX 文本） | binary 标注 | — | — | — | copy/··· 不画维持 |
| IN-06 | Summary 字段文本 | 缺失显 unknown | — | — | — | — |
| IN-07 | terminal + terminal-output / terminal-input / terminal-start / terminal-resize / terminal-back-to-bottom | idle/creating/ready/stale/failed；断线禁用 | ✓ | ✓ resize/start/back-to-bottom 根级 arm（同源 terminal_can_operate / terminal_start_enabled gate）+ terminal-input Enter 写入 | ✓ AXPress + output value | r6b T1 idle/ready；R6B 收口四 dir（u2-final-4） |
| IN-08 | resources-refresh + 列表 | 当前页；failed 红字 | ✓ | ✓ 根级 arm | ✓ | — |
| IN-09 | session mismatch 提示行 | 条件显示 | — | — | — | 文本提示，无动作 |
| SB-01 | status-bar + run-status | 常驻；缺权威值显 — | — | — | — | partial 维持（tokens/quota/tok/s 无来源） |
| SB-02 | （触发器已迁出） | — | — | cmd-i 全局 ✓ | — | **已过期**：R6A 已把 Activity 触发器迁入 Workspace header inspector-toggle（折叠态）＋ inspector-collapse（展开态），StatusBar 只剩状态文本；D-01 位置冲突行作废 |

### 浮层（PO-01）

| ID | AX identifier | 状态集 | mouse | keyboard | AX | manifest 修订 |
| --- | --- | --- | --- | --- | --- | --- |
| PO-01 | activity-popover + activity-changes-heading + activity-open-changes | 折叠态可开；单开互斥 | ✓ 触发器 click（r6a） | ✓ inspector-toggle Enter on_activate（mod.rs:710）+ 菜单 Enter 选择 | ✓ AXPress（r6a action-press-inspector-toggle / action-press-activity-open-changes） | **锚定已修正**：header 右上触发、向下展开（D-01 落地）；Agents 分区 honest-hidden 维持 |

## 3. 本波（R7 Wave A）三路径差额修复

1. **菜单双焦点（TR-01/TR-03/CP-03/CP-11 触发器）**：修复前菜单打开时触发器与高亮项同时发布 focused=1（r7 首跑取证：[u2-three-path-dual-focus-gap](u2-three-path-dual-focus-gap/)）。修复：open_menu.is_none() && trigger_focus.is_focused(window)，AX 焦点唯一移交高亮项（accessibility/app.rs:396/416/445/472）。
2. **CP-11 WorkspaceConfirm 高亮体系**：menu_item_count / menu_selected_index 纳入 WorkspaceConfirm，浮层行接 focused(ix == highlight)，Enter 打开后 ↑/↓ 可选、Escape 回焦 add-task。
3. **TL-04 Review changes / TL-07 EntryMenu 键盘激活**：行级 on_activate 与 click 同 handler 同 enable gate（timeline_entry.rs:235/551），键盘复核 event_id 防虚拟化越权。
4. **TR-06 Reconnect 焦点接线收口**：reconnect_focus 字段 + Tab 档 -17 + AX focused 同源。
5. **TR-07 add-task 焦点口径收口（复跑通过）**：driver 断言同步为「workspace-confirm 存在 + 唯一焦点在高亮项」后，u2-three-path-fixed 第三轮全相位通过（Tab 链 / 裸 Enter 开 grouping·workspace-confirm·scope / 菜单键盘 / cycling / 断线重连 / blocked·unread）。

## 4. 遗留 gap（不阻塞本波，登记待后续）

- **TR-11 Unassigned 桶**：无 fixture 样本行，三路径结论依赖源码同源；建议后续 fixture 增样本（涉 crates/app fixture 数据，超出本波写入集）。
- **CP-06/CP-07/TL-03/IN-05/SB-01**：结构性 partial 维持（无 wire 来源 / 视觉重构目标），见 manifest §7。
- **AX server 注册 flake**：macOS 26.6.2 平台时段性递归 AXApplication 劣化（2026-08-29 20:23 起持续，26 层递归链、无 session-list、仅系统菜单 identifiers，desktop-restart 不恢复；state-supplement-attempt7–10 fail-closed 归档）。A3 bundled/签名对照已通过（ax-forms/：raw 46 / bundled-signed 63 Pawork identifiers，required 五项全中，无 AXWindows 回退）；State A 补充采集待劣化窗口过后重跑 scripts/ui-r7-wave-a-states.sh。
