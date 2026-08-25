# State A 量图 checklist（Wave A）

> 口径：docs/UI_Review.md §0.1 六层级 + §7 证据包。本清单评价的是 reference 量图包本身的完备性与已登记冲突；交互与 AX 项按 brief 一律 BLOCKED（等 Wave C/D）。实现侧验收在后续波次以本包数值为准。

## 1. 结构与状态（§0.1 L1）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 区域齐全：TaskRail / Workspace / Inspector / Header / Timeline / Composer / RunStatusBar 全部可见且顺序正确 | PASS | [measurements.md §1](measurements.md) 区域几何表 |
| Inspector 展开态；Changes 顶层页签选中 + Files 二级页签选中 + 选中文件 + DiffView | PASS | [inspector-toptabs.png](crops/inspector-toptabs.png) / [inspector-subtabs.png](crops/inspector-subtabs.png) / [inspector-files.png](crops/inspector-files.png) |
| Timeline 含 Header 下方连续内容：用户/助手条目、工具活动组、完成摘要 | PASS | [timeline-entries.png](crops/timeline-entries.png) / [tool-activity-group.png](crops/tool-activity-group.png) / [run-summary-card.png](crops/run-summary-card.png) |
| 深色整窗、traffic lights 内嵌内容视口（顶部条带 y0–31，灯位见量图） | PASS | [traffic-lights.png](crops/traffic-lights.png) |
| State A 无 ActivityPopover（Inspector 展开态） | PASS（N/A 确认） | 区域几何表 |

## 2. 主几何（§0.1 L2）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| TaskRail 位置与宽度已量化 | PASS | 297px（换算原画布 306.5；与 §2 定稿 288 冲突已登记） |
| Inspector 位置与宽度已量化 | PASS | 460px（474.7；与 ~440 冲突已登记） |
| Workspace / Header / Timeline / Composer / RunStatusBar 位置尺寸已量化 | PASS | measurements.md §1/§2 |
| 分隔线贯通且无双线 | PASS | [rail-border.png](crops/rail-border.png) / [insp-border.png](crops/insp-border.png)（单条 2–3px） |
| 定稿值冲突处理 | PASS（已裁定） | 2026-08-26 用户确认几何以 design §2 文档值为实现合同；图像偏差由分区 reference/current 矩形、anchor 与 min_coverage 表达，不反改合同 |

## 3. 组件几何（§0.1 L3）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| TaskRail 标题/筛选/连接/列表/账户各区槽位、行距、图标位 | PASS | measurements.md §2.1（行距 43–44，角标 29×28，r≈2–3） |
| Header 标题/branch/状态/动作按钮 | PASS | §2.2（标题 cap17，按钮 40×37） |
| Timeline 条目/正文行距/工具组/摘要卡/按钮 | PASS | §2.3（正文行距 24，工具行距 54，按钮 168×40/168×39） |
| Composer 面板/输入区/控件行/ContextMeter/Send | PASS | §2.4（面板 621×98，Send Ø35–36，进度条 90×3） |
| RunStatusBar 高度/文字带/分隔线 | PASS | §2.5（内区 35px，文字 y1002–1013，分隔线 x954/1134/1282） |
| Inspector 两级 tabs/统计行/Files 行/diff 行/页脚 | PASS | §2.6（Files 行距 43，diff 行距 20–21，等宽 cap10） |
| 与 §2 定稿控件尺寸对照 | PASS（已裁定） | 控件高 28–30、Send 32 仍为实现合同；图中 37 / 35–36 只作近似视觉参考 |

## 4. 视觉语言（§0.1 L4）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| surface 层级（bg/边框/raised/选中面）已量化 | PASS | §4（rail/ws/insp 底 #06121a/#061219/#06121a；Files 选中面 #141d23） |
| 文字层级（标题/正文/次级/等宽）已量化 | PASS | §3 字阶表（cap17/16/12–13/10） |
| accent/语义色已取样并对照 token | PASS | §4（蓝系 #1256e4–#4172f5，绿系 #71b13d–#79bb45，−24 #ca6647） |
| 冻结 token 目标 | PASS（已裁定） | 三态实测与对比度约束已冻结到 design §2.1；R1 前生产值只保留为测量对照，R2 负责落源码与真实组合色复验 |
| 圆角/描边量测 | PASS | §2（角标 r≈2–4，面板 r≈4–5，diff 卡 r≈12–13，按钮 r≈8–10，均 ±1–2px） |

## 5. 交互（§0.1 L5）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| hover / active / focus / 菜单 / 折叠 / 滚动 | BLOCKED | 静态图无法取证；Wave C/D 处理（brief 约定） |
| AX（VoiceOver / 键盘路径 / 目标尺寸） | BLOCKED | 同上 |

## 6. 区域量化（§0.1 L6）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 动态值遮罩清单（只遮值） | PASS | [mask.json](mask.json)（80 项，含 4 条窗缘 reference artifact；多行正文与 Diff 均逐行紧遮） |
| 容器/基线/密度/状态图标不遮 | PASS | mask 各条 reason 已注明保留项；gutter、+/− 前缀、行底语义色、状态点、✓、标签词均未遮；任一 zone 遮罩 ≤35% |
| 分区 SSIM ≥ 0.99 | BLOCKED | 需同 fixture 的 current 截图；Wave A 仅 reference 量图 |

## 7. 证据包清单（§7）

| 证据 | 状态 | 说明 |
| --- | --- | --- |
| reference.png | PASS | 1440×1024 归一定稿图（已存在） |
| measurements.md | PASS | 本包量图报告（六章节） |
| crops/（20 项） | PASS | 组件级证据 crop（含 2–4x 放大） |
| mask.json | PASS | 动态值遮罩 |
| checklist.md | PASS | 本文件 |
| current.png | BLOCKED | 等后续波次固定 fixture 截图 |
| overlay-50.png | BLOCKED | 依赖 current |
| diff-heatmap.png | BLOCKED | 依赖 current |
