# R3 Wave B · U2 真窗口键盘导航驱动（Slice 3 缺口取证 → Slice 4 修复收口）

- 运行：`scripts/ui-r3-wave-b-nav.sh run --out <dir>`（seed → serve → desktop → CGEvent HID 键盘注入 → AX 相位断言 → 截图归档）。
- 本目录为 **Slice 4 修复后全量通过** 的证据（2026-08-27，label `r3-wave-b-u2-nav-slice4`，22 相位 structural_pass=true）；Slice 3 的缺口取证基线（19 相位，含 `tab-no-traverse` / `enter-gap` 负向发现）完整移至 [`slice3/`](slice3/)。

## Slice 4 修复的两个缺陷（Slice 3 取证）

1. **Tab 不走焦点链（slice3/enter-gap.json 同批的 assert-tab-no-traverse.json）**
   - 根因：GPUI 无默认 tab cycle；macOS 上 NSWindow 在 sendEvent 层把裸 Tab 送进 key-view 循环（本窗口单一 GPUI 视图、循环为空），事件被静默吞掉，根节点 keyDown 收不到。旧 API `setAllowsKeyboardNavigation:` 已被现代 macOS 移除（NSInvalidArgumentException → nounwind 边界 abort，见 22:15 crash report）。
   - 修法：`install_appkit_tab_monitor`（AppView 首帧一次性安装）——手写 Apple block ABI 的 NSEvent 本地监听器，在 NSWindow 派发前截获裸 Tab / Shift-Tab（keyCode 48，cmd/ctrl/alt 组合放行），经 thread_local `AsyncWindowContext` 调 `window.focus_next()` / `focus_prev()`；监听器体 catch_unwind 防 nounwind abort。composer `TextInput` 挂 `COMPOSER_TAB_INDEX=1` 作链尾（wrap 回 rail 首停）。根节点 on_key_down 的 Tab 分支保留作后备。
   - 证据：`assert-tab-traverse-scope/grouping/add-task.json`（composer → project-scope → task-rail-grouping → add-task）与 `assert-tab-reverse-grouping.json`（Shift-Tab 反向）。
2. **聚焦 ListRow 上 Enter/Space 不触发（slice3/enter-gap.json，enter_gap=1）**
   - 根因：GPUI 对聚焦 stateful 元素的 keyboard click 挂在 keyup 合成路径，真窗口注入不可达。
   - 修法：`ListRow::on_activate` 行级 `on_key_down` 裸 Enter/Space 直接调与 click 同一激活 handler（task 行 `on_session_clicked`、项目头 `on_toggle_project`），禁合成 click；`pending_row_key_activate` 衔接标记吞掉物理键盘同键 keyup 合成 click 防双触发。
   - 证据：`assert-key-open-task.json`（正向 Enter 打开聚焦行，无 cmd-alt-down 兜底）。

## 相位口径变化（19 → 22）

`tab-no-traverse`（负向）翻转为 `tab-traverse-scope/grouping/add-task` + `tab-reverse-grouping` 四个正向相位；`enter-gap` 取证撤出主流程由 `key-open-task` 正向断言替代；其余 18 个相位口径不变（rail 焦点链 / 菜单键盘 / cmd-alt 循环 / 断线重连 / Blocked·Unread live）。

## 驱动工具链修复（本轮排查追加）

- **ui-key-event.swift flags 强制赋值**：`CGEventSource(.hidSystemState)` 创建的事件继承源当前 flags——cmd-alt-n 投递后 cmd|alt 粘滞在 HID 状态，后续裸 Tab 以 0x180000 到达，被监听器按「带修饰键组合」放行后遭 AppKit 吞掉（run2/run3 首相位卡死根因）。修复：`event.flags` 无条件赋成请求值。
- **key()/click_id() 发送前 soft_activate**：真机前台可能被用户/其它应用抢走，CGEventPost 走全局 HID tap 只送达前台应用；重试路径不激活会让按键全部漏发。
- **RUN_TIMEOUT_SECS watchdog**（默认 900s，`PAWORK_UI_RUN_TIMEOUT_SECS`）：超时记录失败相位并走 teardown 清理退出，禁无限等待。

## 证据清单

- `run-manifest.json`：22 相位 pass 明细（git_head 69d1fb3）。
- `assert-<phase>.json` × 22：全部 pass=true。
- `ax-tree-<phase>.txt` / `shot-<phase>.png` × 11：关键相位 AX 树与截图。
- `action-trace.txt`：逐相位时间线（含每步按键投递日志）。

## Slice 5（2026-08-28 审查修复收口）

本目录 **U2 证据仍是 Slice 4 的 22 相位**（label `r3-wave-b-u2-nav-slice4`，
`run-manifest.json` 与 `assert-*.json` 未覆盖 button-enter 新相位；不存在
`slice4/` 归档目录）。Slice 5 代码修复已落地并通过 `cargo test` / python
单测；按 2026-08-28 用户指示，快捷键方向不再过度复跑驱动。驱动脚本已含
button-enter 正向相位，后续复验以 Computer Use 为准。

### 修复清单与根因

1. **P1 BLOCK_IS_GLOBAL 位错误（mod.rs install_appkit_tab_monitor）**
   - 根因：块 flags 误写 `1<<30`（BLOCK_HAS_SIGNATURE，Objective-C 类型编码
     位）；Apple block ABI 全局块应为 `1<<28`。对照
     `~/.cargo/registry/.../block2-0.6.2/src/abi.rs`：BLOCK_IS_GLOBAL = 1<<28、
     BLOCK_HAS_SIGNATURE = 1<<30。监听器块不设签名域，descriptor 为最小布局
     reserved + size（无签名域），注释一并对齐实际 ABI。不引新 crate。
2. **P2a cycle_active_task 目标态重开会话（mod.rs）**
   - 根因：单会话 rail 环绕时 target==active，旧实现仍 open_session 重开，
     导致重新分页 timeline 与 Composer 失焦抖动。修复：与 on_session_clicked
     的 active 短路一致，target==active 直接 return（cycling 语义 no-op 安全）。
3. **P2b rail 聚焦 Button 的 Enter/Space 无激活路径**
   - 根因：GPUI 对聚焦元素的 keyboard click 挂在 keyup 合成路径（同 Slice 4
     enter-gap）；Button 只有 on_click 挂载，真窗口裸 Enter 在 keydown 层无
     handler。修复：Button 补行级 `on_activate`（与 click 同 handler 路径，
     禁合成 click），4 个 rail 触发器（grouping / scope / add-task / 项目定向
     「+」）挂接；激活时记 `pending_button_key_activate` 标记，on_click 内先
     `consume_button_key_click` 吞掉同键 keyup 合成 click（防菜单闪关 / 重复
     建稿）。
4. **P3a 菜单键盘接管去硬门控（mod.rs + task_rail.rs）**
   - 根因：旧实现菜单 ↑/↓/Enter 以 menu_trigger_focused 硬门控；widget
     路径（真点击开菜单时触发器未聚焦 / 行级元素聚焦时）按键落到别处。修复：
     Grouping/Scope/Model 菜单打开即接管 ↑/↓/Enter（根节点 handle_root_key，
     P3b 行级 on_activate 与 rail 导航在菜单打开时让位不 stop，事件冒泡到根
     节点）；Escape 关闭一律 close_menu_and_focus_trigger 回焦触发器（不要求
     触发器原本聚焦）。与 spec §3.3 对齐。
5. **P3b 键盘合成 click 跨行误触发（mod.rs consume_row_key_click）**
   - 根因（布尔反转教训）：旧实现按行键匹配标记，标记落在他行时该行会把
     键盘合成 click 当真点击误激活。修复：判定收归自由函数
     `should_swallow_keyboard_click(keyboard_click, marker)`——键盘 click +
     标记存在即吞（行键 / 按钮 id 不参与匹配，标记不匹配视为陈旧一并消费）；
     鼠标 click（有按下位置）永不吞、不动标记。新增 Rust 单测
     `keyboard_click_swallow_disregards_marker_identity` 钉住六种组合。

### 相位口径变化（驱动脚本 22 → 26；本目录归档仍为 22）

驱动脚本与 `ui-r3-wave-b-tools.py` 已增加 `tab-reverse-scope` 与
`button-enter-scope-menu` / `button-enter-grouping-menu` /
`button-enter-add-task-popover` 正向相位。本目录归档证据仍为 Slice 4 的
22 相位；上述新相位本轮未复跑，不作为本波通过门禁。

### 证据清单（本目录实际归档）

- `run-manifest.json`：22 相位 pass 明细（label `r3-wave-b-u2-nav-slice4`，
  git_head 69d1fb3）。
- `assert-<phase>.json` × 22：全部 pass=true。
- `ax-tree-<phase>.txt` / `shot-<phase>.png`：Slice 4 关键相位 AX 树与截图。
- `action-trace.txt`：Slice 4 逐相位时间线。
