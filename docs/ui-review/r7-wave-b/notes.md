# R7 Wave B — 全局 focus、菜单与 Popover 等价路径

> 状态：🟢 已关闭（2026-08-30）

## 目标与边界

- 目标：基于当前源码、Wave A 组件矩阵和既有真窗口证据，核对全局 focus、单开浮层、dismissal、快捷键及 mouse / keyboard / AX action 等价性；只修复当前可证明的缺口。
- 非目标：不改 GUI wire、Host、Policy、storage、fixture 业务数据或设计 reference；不开始 Wave C 的 1080×720、长内容、平台偏好与性能工作；不重做 R10 三图视觉终局门禁。
- VoiceOver 边界沿用用户决定：Wave A 以原生 AX tree/action + 纯键盘 + U2 替代该波 VoiceOver。VoiceOver 仍未执行、不记为通过，屏幕朗读措辞 / 顺序未验证，R10 系统级口径不自动豁免。

## 事实基线与审计结论

- 继承证据：Wave A [`component-matrix.md`](../r7-wave-a/component-matrix.md) 与 [`u2-three-path-fixed/`](../r7-wave-a/u2-three-path-fixed/)；R6 Inspector/Activity 最终 U2 [`u2-reviewfix-pass-20260830/`](../r6-wave-b/u2-reviewfix-pass-20260830/)。
- 已有合同无需重做：`Option<MenuKind>` 保证浮层单开；选择、再点触发器、Escape、外点均有关闭路径；菜单方向键/Enter 与快捷键集中登记；Inspector 折叠/展开已有 `pending_inspector_focus`；AX action 会重取当前树并回到与可见路径相同的 handler / gate。
- 首轮审计发现四个真实焦点缺口：任务切换可能保留旧菜单；审批按钮将随卡片卸载但未交接焦点；Run Summary 的 Review changes 未聚焦已选 Changes tab；Fork 接受后焦点可能留在即将卸载的条目菜单触发器。
- 独立代码审查再发现两个同合同边角：菜单打开时 AXPress 当前已选 task 会命中 `on_session_clicked` 提前返回而遗留菜单；rail 仅一个可见 task 时 cycling 环绕回当前索引会提前返回而不交接焦点。两项均可复现，按 P1/P2 补入本波，不另起抽象。

## 最小实现

1. `open_session` 先关闭旧菜单；任务行 click / AX press 与 task cycling / next-needs-attention 快捷键均在切换后聚焦 Composer。激活当前 task 与单可见 task cycling 不重开 session，但仍关闭旧菜单并把焦点交回 Composer。
2. 三个审批入口继续复用 `can_approve` 与同一 `on_approve`；命令被接受后关闭旧菜单并聚焦 Composer，避免焦点悬挂在卸载的审批按钮。
3. Review changes 展开 Inspector 时登记 `SelectedTab` 焦点目标，使展开后焦点落在当前选中的 Changes 顶层页签。
4. Fork 继续复用既有双重 enable gate；命令被接受后聚焦 Composer。

## 验证与真实证据

- `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`：**144 passed / 0 failed**（3 组既有 warning）。
- `python3 scripts/test_ui_r3_wave_b_tools.py`：**17/17**。
- `/tmp/pawork-wave-d-venv/bin/python scripts/test_ui_r4_wave_b_states.py`：**22/22**。
- 导航 U2：[`u2-nav/run-manifest.json`](u2-nav/run-manifest.json) `structural_pass=true`，26 相位全绿；新增断言直接钉住 task Enter、Cmd+Opt+↑/↓、Cmd+Opt+N 及 grouping 选择后的唯一焦点。
- 审查边角定向 U2：[`u2-focus-edge-isolated/run-manifest.json`](u2-focus-edge-isolated/run-manifest.json) `structural_pass=true`，3 相位全绿；`ax-current-task-menu` 证明菜单打开时 AXPress 当前 task 后菜单消失且唯一焦点为 Composer，`single-visible-before-cycle` / `single-visible-cycle` 证明仅一个可见 task 时快捷键不重开 session、焦点从 scope 触发器交回 Composer。该模式只在临时 fixture root 的 `sessions.archived` 字段归档其余 seed session，变体清单保存在 [`focus-edge-fixture-state.txt`](u2-focus-edge-isolated/focus-edge-fixture-state.txt)，未改仓库 `fixtures/ui/seed.json`。
- 审批/状态 U2：[`u2-approval-rerun/run-manifest.json`](u2-approval-rerun/run-manifest.json) `structural_pass=true`，14 相位全绿；`approval-resolved` 直接断言审批卡消失、既有状态收敛且唯一焦点为 `composer-input`。
- 首次审批长驱动保留于 [`u2-approval/`](u2-approval/)：审批目标相位已经通过，随后在 S8 因旧 R4 断言要求空输入 Send enabled 而失败（[`assert-hang-cancelled.json`](u2-approval/assert-hang-cancelled.json)）。R5 冻结合同明确空/纯空白 Send disabled；只校准测试口径后，同一驱动复跑全绿，没有改产品行为或隐藏首次失败。
- 审查边角补录的探索性失败同样保留：`u2-nav-final*` / `u2-nav-reviewfix-pass*` 已先证明 `ax-current-task-menu`，但旧完整驱动无法构造单可见 task；`u2-focus-edge*` 的前序尝试分别暴露未持久化 draft、workspace 共享折叠 key 与非 git gamma 不进入可创建 workspace 列表。最终改为隔离运行时 archive 变体后一次通过；这些失败都发生在目标 cycling 动作之前，不冒充产品失败或通过。
- 独立复审确认 P1/P2 修复与 focus-edge 证据一致，无阻塞项；trace 中按键前的一次 `TIMEOUT` 是预期失败的先查探针，后续真实按键注入通过，不是运行失败。

## 关闭决定

- 已实现：六个焦点交接缺口均收敛，mouse / keyboard / AX 继续复用同一 handler 与 enable gate。
- 自动门禁：Desktop 定向测试、两组 Python 断言、两套主 U2 与审查边角定向 U2 均通过。
- 人工验收：本波没有新增独立视觉签字要求；Wave A 九图签字保持有效，但不替代 R10 三图 SSIM。
- 结论：Wave B 关闭；当前指针进入 Wave C。VoiceOver、响应式/长内容、平台偏好和 R10 视觉终局仍未完成。
