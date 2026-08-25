# State B checklist（UI_Review §0.1 六层）

本 checklist 只评估 State B reference 的量图与合同一致性，不冒充当前实现验收。PASS 表示量图证据支持；BLOCKED 表示缺少运行证据；N-A 表示该状态不适用。几何与色板冲突已经裁定，仍在 measurements 中保留原始实测差异。

| 层级 | 结论 | 证据 / 缺口 |
| --- | --- | --- |
| 结构与状态 | PASS | [taskrail.png](crops/taskrail.png)、[workspace-header.png](crops/workspace-header.png)、[activity-popover.png](crops/activity-popover.png) 显示 TaskRail、折叠 Inspector 后扩展的 Workspace、右上两动作槽、右上锚定 Popover、Timeline、Composer、StatusBar。Inspector 列宽为 0。 |
| 主几何 | PASS（合同已裁定） | 实测偏差保留在 measurements §5 C-01..C-04；2026-08-26 用户确认 design §2/§5.1 的 288 / 约320 / 88–94 / 24 为实现合同，图像偏差由 zone anchor/min_coverage 表达。 |
| 组件几何 | PASS（静态合同） | 动作槽、锚点、Header branch/终态、Popover 内距/行高、Composer footer、StatusBar 均已量化；8px 混合值只作图像参考，C-05 误量已纠正。真实交互仍由后续波次验证。 |
| 视觉语言 | PASS（静态合同） | 深色层级、边框、文字层级、状态色和图标语义已取样；C-06 已按三态实测与对比度重定到 design §2.1。hover/active/focus 的运行行为仍在交互层 BLOCKED。 |
| 交互 | BLOCKED | Wave A 静态图不能证明 hover、active、focus、Escape/外点、滚轮 occlude、 popover 收起或摘要点击展开 Inspector。锚点几何本身为零遮挡提供证据：Popover 与 Composer 横向/纵向间隙 100/437 px。 |
| 区域量化 | BLOCKED | State B 已有 reference、mask 与本 checklist；current/overlay-50/diff-heatmap 和分区 SSIM 尚未产生，不能宣称 ≥0.99。 |

### §7 证据包状态

| 证据 | 状态 | 说明 |
| --- | --- | --- |
| reference.png | PASS | 1440×1024；无 ICC 的 RGB 按 sRGB 解释后归一。 |
| current.png | BLOCKED | Wave A 不生成当前实现截图。 |
| overlay-50.png | BLOCKED | 等待同 fixture current。 |
| diff-heatmap.png | BLOCKED | 等待同 fixture current。 |
| mask.json | PASS | 58 项（含 4 条 reference artifact）；动态正文逐行紧遮，Header branch 图标/终态、容器、行位、图标和留白未遮；zone 遮罩 ≤35%。 |
| checklist.md | PASS | 本文件；交互/AX 按 Wave A 口径阻塞。 |

### State B 特别核对

- ActivityPopover 锚点：PASS。触发器 x=1376..1415、y=48..87；指针 x=1387..1403、y=87..95；Popover 右缘与触发器右缘对齐，顶边 y=94。
- 不覆盖 Composer：PASS（几何证明见 measurements §1；横向间隙 100 px、纵向 437 px）。
- 摘要可恢复 Inspector：BLOCKED，静态图不可证明点击行为。
- StatusBar 不覆盖 TaskRail 账户区：PASS。StatusBar 从 x=320 开始，TaskRail 账户区在 x=0..319；二者仅共享 y 带但横向不相交。
