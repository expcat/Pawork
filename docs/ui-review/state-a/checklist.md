# State A 量图 checklist（Wave A）

> 口径：docs/UI_Review.md §0.1 六层级 + §7 证据包。§1–§4 评价 Wave A reference 量图包本身的完备性与已登记冲突；§5–§7 已按 Wave D 真窗口取证更新，完整交互/AX 与视觉还原仍由 R2–R8 继续。实现侧验收以本包数值为准。

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
| hover / active / 菜单 / 折叠 / 滚动 | BLOCKED | State A Wave D 只覆盖 task AXPress 与 Composer focus；完整状态矩阵留 R2–R8 |
| AX（VoiceOver / 键盘路径 / 目标尺寸） | PARTIAL | Wave C/D 已证明真窗口 AX tree、task semantic action 与 focus；完整 VoiceOver/键盘/目标尺寸留 R7/R8 |

## 6. 区域量化（§0.1 L6）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 动态值遮罩清单（只遮值） | PASS | [mask.json](mask.json)（80 项，含 4 条窗缘 reference artifact；多行正文与 Diff 均逐行紧遮） |
| 容器/基线/密度/状态图标不遮 | PASS | mask 各条 reason 已注明保留项；gutter、+/− 前缀、行底语义色、状态点、✓、标签词均未遮；任一 zone 遮罩 ≤35% |
| 分区 SSIM ≥ 0.99 | FAIL（留 R2–R6） | Wave D 已生成同 fixture current；0/9 zones 通过，global 辅助 SSIM 0.336185，详见 §8 |

## 7. 证据包清单（§7）

| 证据 | 状态 | 说明 |
| --- | --- | --- |
| reference.png | PASS | 1440×1024 归一定稿图（已存在） |
| measurements.md | PASS | 本包量图报告（六章节） |
| crops/（20 项） | PASS | 组件级证据 crop（含 2–4x 放大） |
| mask.json | PASS | 动态值遮罩 |
| checklist.md | PASS | 本文件 |
| current.png | PASS（证据存在） | Wave D baseline-1 的无 ICC `RGB` 1440×1024 真窗口截图；不表示视觉门禁通过 |
| overlay-50.png | PASS（证据存在） | reference/current 50% overlay；视觉差异仍明显 |
| diff-heatmap.png | PASS（证据存在） | 全图差异热力图；分区结果见 diff-report.json / zone-evidence/ |

## 8. Wave D 真窗口闭环（2026-08-27）

Wave A 的静态量图结论保留在上文；下表追加实现侧 U2/U3 证据，不把当前视觉偏差改写成已还原。

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 真 Host / 真 Desktop / fixture / `timeline_stable` / AXPress task | PASS | [baseline-1 manifest](../wave-d/state-a/baseline-1/run-manifest.json) 与 [action trace](../wave-d/state-a/baseline-1/action-trace.txt) |
| 三栏骨架、1440×1024、rail 288、Inspector 440、StatusBar 24、task 选中、Timeline 加载、Composer focus | PASS | [baseline-1 checklist](../wave-d/state-a/baseline-1/checklist-current.md)；Composer 实测 156px 为 F-09 已知视觉偏差，只在 R1 驱动门禁中记为 `OBSERVED-FAIL`，R2 仍须修到 88–94px |
| 主显示器位置与截图色彩归一 | PASS | [window placement](../wave-d/state-a/baseline-1/window-place.txt)；截图 embedded ICC 显式转换到 sRGB 后输出无 ICC RGB，见 [normalize.json](../wave-d/state-a/baseline-1/normalize.json) |
| 两次从零基线可重复 | PASS | `current.png` 字节一致，zone/global 数值指纹完全一致；[repeatability.json](../wave-d/repeatability.json) |
| 故意把 `SIDEBAR_WIDTH` 288→320 | EXPECTED FAIL | 驱动退出 4；初始/最终 `rail-width` 均 FAIL，完整证据见 [drift manifest](../wave-d/drift/run-manifest.json) 与 [drift comparison](../wave-d/drift-detection.json) |
| token 恢复 288 后复验 | PASS | 恢复截图与 baseline 字节一致、指纹一致；[recovery comparison](../wave-d/recovery-compare.json) |
| State A 分区 SSIM ≥0.99 | FAIL（留 R2–R6） | 0/9 zones 通过，global 辅助值 0.336185；规范产物：[current.png](current.png) / [overlay-50.png](overlay-50.png) / [diff-heatmap.png](diff-heatmap.png) / [diff-report.json](diff-report.json) / [checklist-current.md](checklist-current.md) |
