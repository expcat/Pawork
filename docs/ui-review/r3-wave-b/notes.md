# R3 Wave B 收口记录（2026-08-28）

> 范围：任务书范围 5/6（导航状态与键盘）+ Unread/Blocked 状态源，按 [plan/R2-R3-ui-shell-navigation.md](../../../plan/R2-R3-ui-shell-navigation.md#r3--taskrail-与任务导航)。Wave A（F-03/F-04 静态结构）证据见 [../r3-wave-a/](../r3-wave-a/)。
> 语义合同：Blocked = 该 session 最近一条 RunChanged 为 failed/interrupted 终态（live 派生，快照无此 wire 字段故重建后清空，Replay 可再派生）；Unread = 非 active session 收到 Session-stream 活动事件（打开即清，首连/快照不伪造已读）。两轴独立，状态点优先级 NeedsInput > Running > Blocked，Unread 只提标题字重不改几何。

## 1. 实现落点（Slice 1/2/4/5）

- **状态源**（projection.rs）：SessionLiveStatus 增 Blocked；unread 独立通道（session_unread）；断线保留 active/unread/blocked；apply_snapshot_required 保留仍存 active 并清其 unread、消失置 None；顺带修复 membership_changed 覆盖缺陷（|= 化）。
- **键盘**（mod.rs / task_rail.rs / components）：design §3.6 Tab 焦点链（scope→grouping→add-task→项目头/定向+→task 行→composer 链尾，tab_index 档位 + AppKit NSEvent 监听器截获裸 Tab/Shift-Tab）；rail ↑/↓ 移焦、Enter/Space 行级激活（与 click 同 handler，合成 click 以衔接标记吞除）、项目头 ←/→ 展开收起；Grouping/Scope/Model 菜单 ↑/↓ 高亮 + Enter 选择 + Esc 回焦触发器；cmd-alt-up/down task cycling（target==active 短路）、cmd-alt-n next-needs-attention（NeedsInput > Blocked > Unread）；新键入 APP_VIEW_KEYBINDINGS 与空态引导文案（可发现性）。
- **渲染/AX**：Blocked 实心 danger 点、Unread 标题 SEMIBOLD；session_status_description 同源补 Blocked/「· Unread」；新交互补 focused 标志；既有 AX identifier 全部冻结未动。

## 2. 审查与修复

- 审查（deepseek_reviewer，GLM 配额耗尽降级）：P0 无；P1×1（AppKit block flags 1<<30→1<<28，潜在 UB）；P2×2（单会话 cycling 自开整页重拉；rail 按钮 Enter/Space 缺口+无相位覆盖）；P3×2（菜单键控硬门控失焦失效、合成 click 跨行误触发）。
- Slice 4/5 全部修复，逐条根因与证据见 [u2-nav/notes.md](u2-nav/notes.md)（含 Slice 3 缺口取证 → Slice 4 修复 → Slice 5 审查修复的完整链路；U2 归档为 slice3 基线 + Slice 4 的 22 相位）。
- 收口审查（GLM reviewer，2026-08-28）：P0/P1 无；P2×1 焦点回退未实现（已补 `pending_scope_focus`，`on_connected` 无 Window，下一次 render 聚焦 scope）；显示面 tofu（GPUI 默认字体无 ⌘/⌥ 级联，空态/tooltip 改为 Cmd+N / Cmd+Opt ASCII）；P3 文档计数/history/gui-design 与 `on_select_model` enable 门已同批回写。Slice 5 button-enter 相位仍未复跑。

## 3. 验证

```text
Validated:
  cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders → 94/94
  python -m unittest（test_ui_r3_wave_b_tools / test_ui_wave_d_tools / test_ui_r3_wave_a_tools）→ 全绿
  scripts/ui-r3-wave-b-nav.sh run → Slice 4 归档 22 相位 PASS（label r3-wave-b-u2-nav-slice4）；Slice 5 新增 button-enter 相位本轮按用户指示未复跑
  Computer Use 真窗口复核（临时 bundle dev.pawork.desktop，/tmp 已清理）：连接恢复、grouping 菜单、Projects 切换、NeedsInput 会话选中 + 审批卡渲染，截图见 visual/
Targeted regressions: 状态派生/断线保留/快照回退、键位表/焦点链/cycling/attention、合成 click 吞除、AX description 同源扩展
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 4. 已知限制与移交

- **键盘相位深度验证**：按用户 2026-08-28 指示，本轮快捷键不做过度验证。U2 归档证据为 Slice 4 的 22 相位；Slice 5 的 button-enter 正向相位已写入驱动但未复跑。后续复验以 Computer Use 为准（裸 dev binary 无 bundle id，需临时 bundle 包装，本次已验证该路径可行）。
- **快捷键字形**：空态引导与 tooltip 的 ⌘/⌥ 在 GPUI 0.2.2 `.AppleSystemUIFont` 下不级联，真窗口显示为 tofu（修复前截图 [visual/cu-timeline-connected.png](visual/cu-timeline-connected.png)）；收口审查改为 ASCII `Cmd+N` / `Cmd+Opt`。历史 AX dump 不改。本轮父模型无 Computer Use，ASCII 空态复拍标记后续手动启用支持该功能的模型验证。
- **观察（移交 R11 比对）**：task 行标题截断处与时间列之间无可见省略号/间隙（如 "Refactor launcher tab238d"），fixture 长标题样本同样硬截断；属 Wave A 几何既有表现，非本波回归，R11 终局比对时对照定稿图定夺。
- **R3 退出标准（拍板 c 后）**：State A/B/C 分区 SSIM ≥0.99 连同 fixture 演示数据重塑移交 R8；State C reference tone 归一仍须 R8 前用户批准设计基准变更。本波不触及像素门禁。

## 5. 状态

Wave B 已实现、自动门禁与真窗口验证通过、证据归档（[u2-nav/](u2-nav/) Slice 4 的 22 相位 + [visual/](visual/) Computer Use 截图）。2026-08-28 拍板 c 后 R3 阶段已退出；终局视觉签字仍属 R8。
