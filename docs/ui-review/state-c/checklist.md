# State C（Projects）复核清单

口径：docs/UI_Review.md §0.1 六层 + §7 证据包。本波（Wave A）只做量图取证，尚无 current.png；结论只描述 reference 的实测形态，不宣称实现已通过。AX 与交互项一律 BLOCKED（等 Wave C/D）。

## L1 结构与状态

| 项 | 判定 | 证据 |
|---|---|---|
| 三栏顺序 TaskRail → Workspace → Inspector | PASS（图内结构完整） | crops/overview.png |
| TaskRail 标题/Grouping/范围下拉/连接行/全局「+」在场 | PASS | rail-title-grouping.png, rail-scope-dropdown.png, rail-connection-add.png |
| 项目头展开态（Pawork_v2、AsterRoute：chevron 下向 + 任务行） | PASS | rail-project-header-expanded.png, rail-task-rows.png |
| 项目头折叠态（Desklet：chevron 右向，无任务行） | PASS | rail-project-header-collapsed.png |
| 任务行选中态（高亮 + accent dot） | PASS | rail-task-selected.png |
| Workspace Header / 消息 / Tool 卡 / Ready 卡 / Run 行在场 | PASS | ws-header.png, ws-message-block.png, ws-tool-card.png, ws-ready-card.png, ws-run-line.png |
| Composer / StatusBar 在场且不遮挡主操作 | PASS（静态） | composer.png, statusbar.png |
| Inspector 双排 Tab / 摘要 / 文件列表 / Diff / View file 在场 | PASS | insp-tabs.png … insp-viewfile.png |
| 状态集与定稿一致（无缺失/替换 Surface） | PASS（静态可见集） | overview.png |

## L2 主几何（±8px 或 ±1.5%）

| 项 | 实测 | 判定 |
|---|---|---|
| TaskRail 宽 | 283（含 2px 边框 ≈284–285）vs 定稿 288 | PASS（−5，容差内） |
| Inspector 宽 | 465（窗缘）/ 447（内容外沿）vs ~440 | PASS（合同已裁定以 ~440 实现；图像读法保留为证据） |
| Workspace 宽 | 686（285–970） | PASS |
| Composer | x311–936 y863–964（626×102） | PASS（位置）；高度见 L3 |
| StatusBar | y978–1023，h46 vs 定稿 24 | PASS（冲突已裁定：实现合同保持 24，图像偏差由 zone anchor/min_coverage 表达） |
| Popover | 本图无 | N-A |

## L3 组件几何（间距 ±2px，字体/图标/描边/圆角 ±1px）

| 项 | 实测 | 判定 |
|---|---|---|
| 项目头要素 chevron/名称/计数/「+」 | 全部在场，坐标见 measurements §2.1 | PASS |
| 任务行 dot/标题/时间 | 全部在场；行距 50±2 | PASS |
| 选中行高亮 | 252×46，r~4 | PASS |
| Tool/Ready 卡 | 620 宽，r5–6，行高 ~53 | PASS |
| Inspector 文件行距 | 43±1 | PASS |
| Diff 行距 | 20–21 | PASS |
| Composer 高 | 102 vs 定稿 88–94 | PASS（冲突已裁定：实现合同保持 88–94） |
| 8px 基线 | 缩进 15/30/34、间隙 14 等 | PASS（实测偏差已记录；实现仍按 8px 基线与明确组件尺寸） |

## L4 视觉语言

| 项 | 实测 | 判定 |
|---|---|---|
| 深色整窗、无原生标题栏分离 | traffic lights 融入内容区 y15–28 | PASS |
| Surface 层级 | rail/ws/insp 面色与选中面实测见 §4 | PASS（取样已用于 design §2.1 新冻结目标） |
| 文字层级 | 主文本 #dddcd8、次要 #515051–#696869 | PASS（新目标受三态取样与 4.5:1 对比度共同约束） |
| 状态色 | dot 蓝 #2c68f8/绿 #79c951/灰 #848486 | PASS（语义在场；与 token 不强行映射） |
| Accent | 下划线 #2e76ca、Review 渐变 #085cfd→#1e6afb | PASS（§0.2 允许的渐变例外） |
| 图标语义 | 勾圈/绿圈/copy/···/外链等在场 | PASS |

## L5 交互（含 AX）

| 项 | 判定 | 证据/原因 |
|---|---|---|
| hover/active/focus | BLOCKED | 静态图不可验 |
| 菜单/折叠/滚动/主操作位置 | BLOCKED | 等 Wave C/D 真实运行取证 |
| 开合不引发布局跳动 | BLOCKED | 同上 |
| AX tree / hit area / 对比度 / 键盘路径 | BLOCKED | 等真实 UI 复核 |

## L6 区域量化

| 项 | 判定 | 原因 |
|---|---|---|
| 分区 SSIM ≥0.99（mask 后） | N-A | Wave A 仅有 reference.png，无 current.png/overlay/diff |

## §7 证据包状态

| 文件 | 状态 |
|---|---|
| reference.png | 已有（1440×1024 归一） |
| current.png / overlay-50.png / diff-heatmap.png | 未产生（等 Wave C/D 截图与叠图） |
| mask.json | 已有（62 项，含 4 条 reference artifact；Diff 逐行紧遮且 zone 遮罩 ≤35%） |
| checklist.md | 本文件 |
| crops/ | 23 张组件证据 |

硬失败项自查：本图自身无白标题栏、无缺失关键入口、无遮挡、无截断/溢出（静态可见范围内）；Composer/StatusBar 与 8px 基线的图像偏差已按 2026-08-26 仲裁记录，后续实现仍必须满足 design 文档合同。
