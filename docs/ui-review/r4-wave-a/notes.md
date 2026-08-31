# R4 Wave A 收口记录（2026-08-28）

> 范围：F-05 Workspace Header、F-06 Timeline 起始位置（Top 对齐四合同）、F-07 消息层级、F-08 Tool group 与 Run 完成摘要视觉与结构；一轮评审（P0×1 / P1×1 / P2×3 / P3×2）全部修复。交互/状态 U2 场景（审批流、error retry、断线、流式 follow、千级虚拟化）归 Wave B，不在本波。
> 事实源：本目录 state-a/ 证据（r4a-2）与 [desktop Spec](../../spec/crates/desktop.md)；量图合同 [state-a/measurements.md](../state-a/measurements.md) §2.2/§2.3、[state-b/measurements.md](../state-b/measurements.md) §1/§2。

## 1. 实现落点

- **F-05 Workspace Header**（[mod.rs](../../../apps/desktop/src/ui/mod.rs) `workspace_header_element`）：骨架常存 104 高（含 36 traffic-light 安全条），左 inset 28 / 右 25；任务标题 24px SEMIBOLD truncate；branch（⑂ + 17px）仅 `header_branch()` 诚实源——`GitDiffInfo.branch`、有 active session 且无 session_mismatch 才显示（wire WorkspaceSummary 无 branch）；终态 Ø10 点只画 live 可派生 Running / Needs input / Blocked（`SessionLiveStatus` 与 rail 同源），wire 无每会话终态字段不画 Completed 绿点；右侧 40×37 r4 描边 `header-new-task` 与 rail 全局「+」同 handler。
- **F-06 Timeline Top 对齐四合同**（[timeline.rs](../../../apps/desktop/src/ui/timeline.rs)）：自 Bottom 钉底改为 Top 对齐 + 显式跟随——短会话从 Header 下开始不再沉底；`timeline_following` 单一表达跟随态；`sync_list` 跟随臂显式 `scroll_to` 末项底，脱钩读史恢复 reset 前偏移（item_ix 越界钳制）；条目变化仍统一 `reset(count)` 禁 splice。可读列 `TIMELINE_READABLE_WIDTH`=618 左对齐防无限行宽。
- **F-07 消息层级**（[timeline_entry.rs](../../../apps/desktop/src/ui/timeline_entry.rs)）：You/Pawork 标签行（18 medium）+ 时间（17 tertiary，`display_time` 相对词）；正文 18px 行高 24；段落（间 28）与「- 」前缀 • 列表两级切分；条目间 40。
- **F-08 Tool group 与 Run 摘要**（[projection.rs](../../../apps/desktop/src/projection.rs) `TimelineRow` / `timeline_rows()` + timeline_entry 渲染件）：连续同 run ToolCall 合组（r5 描边面板、52 行高、2px 分隔线、状态 ✓/词诚实映射仅 succeeded→Completed、无耗时字段不画）；紧邻同 run 终态吸收为 RunSummary 区域（Ø40 状态圆 + Ready for review + 说明 + 168×40 r8 Review changes 主按钮，Open in editor 无 capability 不画）；终态判定唯一定义源 = `fork_boundary.is_some()` 无字符串匹配；非终态 RunState 归 RunPhase 单行（Interrupted 无 fork 边界同此，不产摘要/页脚）。审批卡仍为 list 末项不占行。
- **Review changes 主按钮**（mod.rs `on_review_changes`）：关菜单 → 展开 Inspector → 切 Changes tab → refresh；Changes unavailable 给 status_hint 原因，不画假可用。
- **AX 同源**（[accessibility/app.rs](../../../apps/desktop/src/ui/accessibility/app.rs)）：`header_ax`（标题/branch/live 状态/header-new-task）与 `timeline_row_ax` 五类行节点，谓词与 metrics 全部与 render 共享；assistant 角色词统一 Pawork；`run-review-changes-*` press 经 Completed 双重把关映射 `on_review_changes`；`timeline-entry`/`entry-menu-*`/`fork-*` 冻结 identifier 原样保留。
- **时间戳同源**（timeline_entry.rs `display_time`）：消息/错误/页脚与 AX description 全部经同一辅助——epoch millis 串 → `relative_activity` 相对词 now/Nm/Nh/Nd，非法串原样兜底（诚实，不伪造）；不引入 tz/chrono 依赖。

## 2. 评审与修复（glm_reviewer 一轮，同批落地）

- **P0 滚动即崩溃**：`install_scroll_follow` handler 内读 ListState 会在 gpui scroll() 写借用存活期重入 `BorrowMutError` panic。修复：贴底判定只用滚动事件事实（`visible_range.end >= count`），handler 不触 ListState。
- **P1 贴底误判**：像素 max 因未测高项系统性低估，长会话读史被误判贴底后遭流式拽底。修复：同上事件事实判定，只在末项可见时重挂跟随。
- **P2 跨 run 吸收**：终态摘要吞并紧邻但不同 run 的 tool 组。修复：吸收前比对 `run_id` 相等；测试改写为同 run 吸收 + 跨 run 拒绝双用例。
- **P2 终态绿 ✓**：Cancelled/Failed 仍画绿色成功圆。修复：`RunSummaryView.terminal` 驱动 Completed 绿 ✓ / Failed danger ✕ / Cancelled —。
- **P2 Review changes 假提示**：refresh 后必报「数据不可用」。修复：先快照可用性，仅真不可用（断连/无 workspace/失败）且非 Fetching 才提示。
- **P3 branch 无会话残留**：无 active session 时仍显示上一会话 branch。修复：`header_branch()` 首行无 session 即 None（render 与 AX 同源修正）。
- **P3 时间戳原样 epoch 串**：见 §1 时间戳同源（`display_time`）。

## 3. 定向回归（全绿）

`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`：**107 passed / 0 failed**（R3 收口基线 94 → 本波 +13：projection 4 / theme 1 / timeline_entry 6→8、含评审修复与 display_time 2）。

## 4. U2/U3 真窗口门禁

- **r4a-1（00:31 +08）**：probe 271 次未就绪——AX dump 只剩 AXApplication 空链、desktop.log 空。当时工作树处于评审修复编辑中断态（编译不过，二进制为中间产物），非环境权限问题（ax_trusted=true、窗口在屏）。修复编译后复跑即恢复，结案为中间态非回归。
- **r4a-2（01:16 +08）**：`scripts/ui-wave-d-state-a.sh run --out docs/ui-review/r4-wave-a/state-a --label r4a-2`，**结构门禁通过（exit 0）**：initial/final 相位断言全 PASS（root 1440×1024、rail 288、Inspector 440@1000、StatusBar 24、shell skeleton、会话选中、timeline-entry 12 条、focus 归 composer）；composer-height 156 为 F-09 已知漂移（R5 范围，blocking=false 登记）。
- **分区 SSIM（记录值，阈值 0.99）**：taskrail 0.694 / header-left 0.940 / header-right 0.883 / timeline 0.665 / composer-left 0.438 / composer-right 0.414 / inspector-body 0.588 / inspector-right 0.740 / statusbar 0.617；global 辅助 0.648。timeline/header 未达 0.99 的主因是 fixture 演示内容与设计稿内容不一致（fixture 数据重塑已随 R3 拍板 c 移交 R10），结构对齐以断言为准；**State A/B 区域 SSIM ≥0.99 的 R4 退出条款已于 2026-08-28 拍板 1 同 R3 先例移交 R10**。
- 证据：[state-a/](state-a/)（current.png、assert-*.json、ax-tree-*.txt、geometry-*.txt、diff/ 分区报告与 heatmap、run-manifest.json、checklist-current.md、action trace、barriers 与日志）。

## 5. 已知偏差与遗留

- Header branch 图标用文本「⑂」（禁 emoji 约束下的字体回退）；与定稿图标的差异归 R8 终局比对登记。
- 时间显示采用相对词（now/Nm/Nh/Nd）而非定稿的绝对钟点（如 10:40 AM）：规避 tz 依赖的拍板，mask 已遮动态时间值；终局一致性归 R8。
- 审批交互、error/retry、断线重放一致、流式 follow、千级虚拟化、State B shell 回归：旧 Wave B 范围；任务书已清理，过程改从 [history](../../history.md) 追溯。
- composer-height 156（合同 88–94）= F-09，R5 范围；State B zones current 映射待 F-12（R6）后补齐。
