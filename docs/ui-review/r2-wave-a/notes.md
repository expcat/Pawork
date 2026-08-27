# R2 Wave A 收口记录（2026-08-27）

> 范围：F-01 Window chrome、F-02 三栏骨架、根级 surface/token 落地、1440×1024 / 1080×720 layout invariant、State A shell 视觉证据。TaskRail 功能扩张（F-03/F-04）等按 ROADMAP 指针不进入本波。
> 驱动：复用 [scripts/ui-wave-d-state-a.sh](../../../scripts/ui-wave-d-state-a.sh)（R1 Wave D 链路不变）。

## 1. 实现落点

- **F-01**：apps/desktop/src/main.rs WindowOptions 配 TitlebarOptions { title: "Pawork", appears_transparent: true }。gpui 0.2.2 mac 实现启用 NSFullSizeContentView + titlebarAppearsTransparent + NSWindowTitleHidden，内容视口贯通全窗、无白带；窗口拖拽由原生顶部条带行为保留。证据：本波 [normalize.json](normalize.json) 窗口帧 1440×1024（R1 基线为 1440×1056 = 1024 内容 + 32 原生标题栏），截图无需裁切即为 1440×1024 内容图。
- **F-02**：新增 [apps/desktop/src/ui/shell_layout.rs](../../../apps/desktop/src/ui/shell_layout.rs) resolve 作为 AppView::render 与 U1 测试共享的唯一壳层几何入口；分隔线 1px 贯通、StatusBar 24 连续（结构门禁 three-column-skeleton PASS，见 [assert-final.json](assert-final.json)）。
- **根级 token**：theme.rs 按 design/README.md §2.1 逐值落地 14 项改值 + placeholder 不透明化 + 新增 semantic.success_fg #74c94c；text.assistant/text.tool 收敛到 emphasis/secondary（timeline_entry.rs 两个消费点）。WCAG 组合定向断言随码落地（theme.rs 三个 wcag_* 测试）。
- **响应式**：窗口宽 ≤1279 时 rail=240、Inspector 强制折叠为 ActivityPopover 抽屉（240+440+560=1240，展开必击穿 Workspace≥560 底线；偏好不改写，加宽自动恢复）。
- **AX 一致性**：accessibility/app.rs 的壳层几何改经同一 shell_layout::resolve，窄窗 AX bounds 不再偏 48px。

审查修复（同批）：
- AX grouping 起点下移 `PAD + TRAFFIC_LIGHT_SAFE_HEIGHT`，首控件不再投影到 traffic-light 带。
- StatusBar AX frame 与 render 同源（不覆盖左栏），窄窗按 `shell.inspector_open` 发布 Inspector 触发器而非偏好值。
- `semantic.success_fg` 接到 TaskRail 运行中任务点，避免 token 只声明不消费。

## 2. 定向回归（全绿）

cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders：74 passed / 0 failed。新增：

- ui::shell_layout::tests::{resolve_switches_rail_width_at_1280, wide_shell_matches_1440_contract, narrow_shell_collapses_inspector_at_1080, resize_collapses_and_restores_inspector}
- ui::theme::tests::{wcag_text_on_surface_pairs_match_frozen_targets, wcag_on_accent_over_hover_actions_match_frozen_targets, wcag_placeholder_stays_below_aa_on_hover_surface}

- ui::theme::tests::success_fg_is_opaque_status_dot_green

## 3. State A shell 视觉证据（本目录）

- [current.png](current.png)：traffic lights 悬浮于深色壳、顶部无白带，色板已切换 v3 深蓝系。
- 结构门禁全 PASS（[assert-final.json](assert-final.json)）；[AX tree](ax-tree.txt) 与 [action trace](action-trace.txt) 随包保存。
- 视觉门禁如实记录：global 辅助 SSIM 0.336185 → **0.650402**；9/9 zone 仍 <0.99（[diff/diff-report.json](diff/diff-report.json)）。这是预期中间态——本波只还原壳层与根 token，内容组件（F-03~F-09）属后续 wave；R2 退出标准要求 State A/B shell SSIM ≥0.99 在 R2 收口时达成。
- 规范包 [state-a](../state-a/) 本波**不同步**：current.png 规范基线在 R2 收口审核后统一替换，本目录即本波完整证据。

## 4. 遗留与候选

- 窗口未设 window_min_size：<800px 时 Workspace 可低于 560（设计合同带宽自 1080 起）。是否钉 min size 由 R2 后续 wave 拍板。
- 1080×720 真窗口响应式功能门禁（D-03）尚未跑真机轮：本波只落了 U1 不变量，U2 resize/focus/blur 模拟操作属 R2 后续 wave。
