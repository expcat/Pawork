# State C（Projects）量图

对象：`docs/ui-review/state-c/reference.png`（1440×1024；源 design/desktop-shell-projects-v3.png 为无 ICC 的 RGB，按 sRGB 解释后直接缩放）。坐标原点左上，x/y/w/h 以参考图像素为准。字号用像素法（文字 crop 亮度阈值行剖面测 cap/x-height → 估算 px），不确定度 ±1px；字重为目测。颜色取平坦区内圈中位 hex。证据 crop 见 `crops/`。

## 1. 区域几何

| 区域 | x | y | w | h | 证据 |
|---|---|---|---|---|---|
| 整窗 | 0 | 0 | 1440 | 1024 | overview.png |
| Traffic lights 条带 | — | 15–28 | 红/黄/绿中心 (22,21)(45,21)(68,21)，d13–14 | — | traffic-lights.png |
| TaskRail | 0 | 0 | 内容 0–282（283px），右边框 x283–284 #282b33 | 全高 | overview.png, rail-title-grouping.png |
| Workspace | 285 | 0 | 686（285–970，右边框 x971–972 #1a1f26） | 全高 | overview.png |
| Inspector | 973 | 0 | 465（973–1437 到窗缘；内容外沿 x1420 时 447） | 全高 | overview.png |
| Workspace Header | 285 | ~28 | 底界无硬分隔线（y80–130 未检出线） | 高度 unknown | ws-header.png |
| Timeline | — | — | — | — | N-A（State C 为 Projects 形态，无 Timeline） |
| Composer 卡 | 311 | 863 | 626 | 102 | composer.png |
| StatusBar | 285 | 978 | 1153（285–1437，顶线 y978 #0b1218） | 46 | statusbar.png |
| Popover | — | — | — | — | N-A（本图无 Popover） |

窗缘亮线（右 x1438–1439 #2a3239/#3f434a、底 y1023 #31353c）为 ImageGen 伪影，非组件。

## 2. 组件量图

### 2.1 TaskRail（rail-title-grouping / rail-scope-dropdown / rail-connection-add / rail-project-header-* / rail-task-* / rail-account）

| 组件 | 几何/间距 | 字号/字重/颜色 | 圆角/描边 |
|---|---|---|---|
| Rail 标题 "Pawork" | x20–88 y56–72 | cap17→估22px，semibold（目测），#bdbcb8 | — |
| GroupingMenu | x200–263 y45–82；folder glyph x211–231 + chevron x242–253 | 图标色随面 | 无可见框/描边 |
| 标题下分隔线 | y96–97 | #15181d | 1–2px |
| 范围下拉盒 | x15–264 y104–135（250×31） | 文字 cap13→估18px | 圆角 ±2 unknown，面 #030b11 |
| 连接行 | dot x21–32 #7bcf4d | — | — |
| 全局「+」 | x234–263 y146–174（30×29） | — | ghost |
| 分隔线 | y185–186 | — | 1–2px |
| 项目头（展开） | Pawork_v2 y210–225 / AsterRoute y462–476；chevron(下向) x15–25、名称 x34 起、计数 x211–219、「+」x234–263 | cap13→估18px | — |
| 项目头（折叠） | Desklet y618–631，chevron 右向 x17–23 | cap13→估18px | — |
| 任务行 | dot x15–25（d10–11）、标题 x34–198、时间右对齐至 x262；行距 50±2 | 标题 #81817f，时间 #565657 | — |
| 任务行选中态 | 高亮 x13–265 y237–282（252×46）#10171c | dot #2c68f8，标题 #939495 | 圆角 ~4 |
| 状态点配色 | 蓝 #2c68f8 / 绿 #79c951 / 灰 #848486 | — | — |
| 组间分隔线 | y491–492 | — | 1–2px |
| 账户区 | 顶线 y924–925；头像圆 x15–53 y939–971（d33）、名称 x55–117、齿轮 x236–253 | 名称 cap→估14px | — |

Rail 底部空白 y632–923（三组任务以下无滚动条/无更多行）。

### 2.2 Workspace（ws-header / ws-message-block / ws-tool-card / ws-ready-card / ws-run-line）

| 组件 | 几何/间距 | 字号/字重/颜色 | 圆角/描边 |
|---|---|---|---|
| Header 标题 | x315–551 y55–71 | cap17→估23px，semibold（目测），亮核 #dddcd8 | — |
| Header meta | branch 图标 x586–600、"main" x612–644、绿点 x665–675 #74c94c、"Completed" x682–749 | cap13→估18px | — |
| Header 右按钮 | x905–944 y45–82（40×38） | ghost | 圆角 5–6 |
| Header 下分隔线 | 未检出（y80–130 无线） | — | — |
| 消息 meta | "You"+时间 x313–436 y129–146 | #515051 | — |
| 消息正文 | x315–816 y166–183，可读宽至 x877（~562px） | cap14/x10→估19px，#b0afac | — |
| 列表行距 | 25–31（~28±3） | — | — |
| Tool 卡 | x312–931 y491–652（620×162），行高 ~53 | — | 边框 #171b22，圆角 5–6 |
| Tool 卡行内 | 图标 x329–346、标题 x363–552、勾圈 x772–787（d16）、"Completed" x796–866、时长 x891–915 | cap13→估18px，#959292 | — |
| Ready 卡 | x312–931 y667–802（620×136） | — | 圆角 ~6 |
| Ready 绿圈 | x328–366（d38） | — | — |
| Ready 标题 | y701–717 | cap17→估23px，#b9b6b5 | — |
| Ready 副文 | 2 行 y738–781 | x9–10→估15px，#5d5b5c | — |
| Review 按钮 | x746–910 y688–728（165×41） | — | 填充 #085cfd→#1e6afb 渐变 |
| Open in editor | x746–910 y736–778 | — | ghost |
| Run 行 | y824–839；左 x315–471、右时间 x868–928 | cap13→估17px，#605e60 | — |

### 2.3 Composer / StatusBar（composer.png / statusbar.png）

| 组件 | 几何/间距 | 字号/字重/颜色 | 圆角/描边 |
|---|---|---|---|
| Composer 卡 | x311–936 y863–964（626×102） | — | 面 #091015；上边框 #1f2429、下边框 #171c20；圆角 4–5 |
| 输入 placeholder | x329–458 y881–898 | →估18px，#57585a | — |
| 模型选择器 | 文本 x327–499 y925–944 | ghost，#79797a | — |
| 附件组 | x489–528 | — | — |
| 工作目录 | x537–688 | — | — |
| ContextMeter | 轨 x712–851 y941–946（140×5–6），填充 x712–796（~61%） | 轨 #2c3032，填充 #375ea1，无文字 | — |
| Send | x883–920 y912–949（38×38） | — | #4f8bfb→#7babfc，胶囊 r~19 |
| StatusBar 文本行 | y994–1008 | cap13→估17px，#5c5b5c/#696869 | — |
| SB 段位 | Task tokens x789–912 ｜ dot x941 ｜ quota x971–1090 ｜ dot x1118 ｜ tok/s x1150–1238 ｜ dot x1271 ｜ Run x1305–1380 | 分隔点 #1a1d24 | — |
| SB 左段 | x300–780 空 | — | — |

### 2.4 Inspector（insp-tabs / insp-summary / insp-filelist / insp-diff-header / insp-diff-code / insp-viewfile）

State C 的 Inspector 与 State A 同形态（双排 Tab + 文件列表 + Diff），以下为逐项实测，未抄 State A 数值。

| 组件 | 几何/间距 | 字号/字重/颜色 | 圆角/描边 |
|---|---|---|---|
| Tab 排 1 | y28–44；"Changes" x1005–1066、"Terminal" x1114–1170 | cap14→估19px | 选中下划线 x987–1086 y53–54（h2）#2e76ca |
| Tab 排 1 右控件 | 竖分隔 x1189–1190、「+」glyph x1210–1224（框不可见）、右 chevron x1367–1380、X x1393–1408 | — | — |
| Tab 排 2 | y81–99；"Files"+徽标 x1015–1072（徽标 d18）、"Summary" x1115–1175 | — | 下划线 x994–1088 y110–111 #3770c1 |
| 摘要行 | y137–149："4 files" x996 起、"+186" 绿 #5f9538、"−24" 红 #bc4733 | digits→估16px | — |
| 文件列表 | 行距 43±1；icon x1006–1020、path x1035–1176（步距 7.7–8.5px/字→估14px）、M 徽 x1387–1407 #7095af | mono 未确证 | 选中高亮 x989–1420 y166–208（432×43）#10171d，r~4 |
| Diff 卡 | x991–1416 y350–~900（426 宽；底缘不确定 ±12，至 y912 线） | — | 边框 #1b1e24/#181d22/#1b2025，r~5 |
| Diff 头 | y367–384：path x1006–1135、绿 M x1317–1328、copy x1351–1364、··· x1385–1398 | — | — |
| Diff 代码 | 行距 20–21；gutter x1009–1024（digits h11→估15px）、代码 x1040+ →估14±2px | 代码 #757474、hunk #595758 | 语义底色 added #061f0f / deleted #1b0e10 / context≈bg |
| View file 行 | x991–1416 y912–960（h49），外链图标 x1378–1392 | — | 顶线 #0d141a、底线 #161a20，无填充 |

## 3. 字阶

| 出现处 | 实测 | 估算字号 | 字重（目测） |
|---|---|---|---|
| Rail 标题 | cap17 | 22px | semibold |
| Rail 列表行（项目头/任务标题/时间/下拉） | cap13 | 17–18px | regular/medium |
| 账户名 | cap ~10 | 14px | regular |
| Header 标题 | cap17 | 23px | semibold |
| Header 元信息 | cap13 | 17–18px | regular |
| 消息正文 | cap14/x10 | 19px | regular |
| Tool 卡行 | cap13 | 17–18px | regular |
| Ready 卡标题 | cap17 | 23px | medium/semibold |
| Ready 卡副文 | x9–10 | 15px | regular |
| Run 行 | cap13 | 17px | regular |
| Composer placeholder | — | 18px | regular |
| Composer 附属行 | — | ~14px | regular |
| StatusBar | cap13 | 17px | regular |
| Inspector Tab1 | cap14 | 19px | regular/medium |
| Inspector Tab2 | — | ~17px | regular |
| Inspector 摘要 digits | — | 16px | medium |
| Inspector 文件路径 | 步距 7.7–8.5px/字 | 14px | regular（mono 未确证） |
| Diff 代码/gutter | 代码 ~cap10、digits h11 | 14±2px / 15px | regular |

全图正文主档集中在 14–19px，标题档 22–23px；未见图内显式 12px 以下文本。

## 4. 颜色取样（实测 vs R1 前生产 token）

| 用途 | 实测 | token | 偏差 |
|---|---|---|---|
| Rail 背景 | #000a10 | sidebar #161616 | 偏暗 |
| Workspace 背景 | #000910 | bg #1e1e1e | 偏暗 |
| Inspector 背景 | #000910 | bg #1e1e1e | 偏暗 |
| Rail 右边框 | #282b33 | border #2e2e2e | 接近 |
| 深分隔线 | #191d24 | border #2e2e2e | 偏暗 |
| 选中面（任务行/文件行） | #10171c | selected #2a2a2a | 偏暗 |
| Composer 面 | #091015 | surface.raised #2a2a2a | 偏暗 |
| 主文本 | #dddcd8/#bdbcb8 | text.primary #e8e8e8 | 接近（亮核略暗） |
| 次要文本 | #515051–#696869 | text.secondary #9a9a9a | 显著偏暗 |
| Accent（选中 dot/下划线） | #2c68f8/#2e76ca | accent.primary #2f6fed | 接近 |
| Review 按钮渐变 | #085cfd→#1e6afb | accent.primary | 接近（渐变端点偏差） |
| 状态绿点 | #74c94c/#79c951 | semantic.success_bg #3d7a4a | 不同用途，未对照 |
| Diff added/deleted 底 | #061f0f / #1b0e10 | semantic.success_bg/danger_bg | 偏暗（语义底极低明度） |

本表是 2026-08-26 色板仲裁的取样证据；新的冻结目标以 [design/README.md §2.1](../../../design/README.md) 为准。渐变/抗锯齿区不取样为代表色。

## 5. 8px 基线核对与冲突表

design/README.md §2 定稿值逐一对照：

| # | 项 | 定稿 | 实测 | 判定 |
|---|---|---|---|---|
| C1 | TaskRail 宽 | 288 | 283（+2px 边框 ≈284–285） | 偏差 −5，±8 容差内 OK |
| C2 | Inspector 宽 | ~440 | 465（窗缘）/447（内容外沿 x1420） | 图像读法 +25/+7；已裁定实现保持 ~440 |
| C3 | Composer 高 | 88–94 | 102 | 图像冲突；已裁定实现保持 88–94 |
| C4 | StatusBar 高 | 24 | 46 | 图像冲突；已裁定实现保持 24 |
| C5 | Popover 宽 | ~320 | 本图无 Popover | N-A |
| C6 | 8px 基线 | 8 的倍数 | 缩进 15/30/34、任务行距 50±2、文件行距 43±1、代码行距 20–21、卡缩进 27/宽 620、Composer–StatusBar 间隙 14 | 图像系统性偏离；已裁定实现继续按合同节奏 |
| C7 | §3.5 项目头要素（chevron/名称/计数/+） | 必备 | 三组项目头全部在场 | 一致 |
| C8 | §3.5 任务行要素（状态点/标题/时间） | 必备 | 全部在场；选中态高亮 252×46 | 一致 |
| C9 | 项目排序 / Unassigned 语义 | 规则见 §3.5 | 静态图不可验 | unknown |
| C10 | 色板 | §2 token | 见 §4 偏差表 | 已按三态取样与对比度重定，目标见 design §2.1 |

## 6. 方法与不确定度

- 字号：文字 crop 亮度阈值行剖面测 cap/x-height，按常规比例估 px，±1px；字重目测，未做字形匹配。
- 颜色：平坦区内圈中位值；渐变（Review/Send 按钮）报端点区间，不取单值。
- 圆角/描边：边缘剖面估计，±1px；范围下拉圆角 ±2 unknown。
- unknown 项：Header 高度（无硬边界）；Diff 卡底缘（±12）；Inspector「+」按钮框（不可见）；等宽字体（未确证）。
- 探针脚本位于 /tmp（r1c_struct/rows/rows2/probe1–10/crops_final.py），可重跑复核；量图只读 reference.png，未改设计基准。
