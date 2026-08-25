# State A 量图报告（Timeline + Inspector 展开）

> 对象：[reference.png](reference.png)（1440×1024；源图为无 ICC 的 RGB，按 sRGB 解释；由 design/desktop-shell-timeline-v3.png 1486×1059 全画布缩放，sx=0.969044 / sy=0.96695）。所有坐标以归一图为准；需与 design/README.md §2 定稿值（原画布单位）对照时附换算值（x÷0.969044，y÷0.96695）。证据 crop 见 [crops/](crops/)。

## 1. 区域几何表

| 区域 | x | y | w | h | 换算原画布 | 证据 crop |
| --- | --- | --- | --- | --- | --- | --- |
| 顶部 traffic-lights 条带 | 0–1439 | 0–31 | 1440 | 32 | — | [traffic-lights.png](crops/traffic-lights.png) |
| TaskRail | 0–296 | 0–1023 | 297 | 1024 | 306.5 宽 | [taskrail-top.png](crops/taskrail-top.png) / [taskrail-list.png](crops/taskrail-list.png) |
| TaskRail|Workspace 分隔线 | 297–299 | 0–1023 | 3 | — | — | [rail-border.png](crops/rail-border.png) |
| Workspace | 300–976 | 0–1023 | 677 | 1024 | 698.4 宽 | — |
| Workspace|Inspector 分隔线 | 977–979 | 0–1023 | 3 | — | [insp-border.png](crops/insp-border.png) |
| Inspector | 980–1439 | 0–1023 | 460 | 1024 | 474.7 宽 | [inspector-toptabs.png](crops/inspector-toptabs.png) |
| Workspace Header | 300–976 | ~28–104（推断，无分隔线） | 677 | ~76 | — | [workspace-header.png](crops/workspace-header.png) |
| Timeline | 300–976 | ~104–873（推断上界） | 677 | ~770 | — | [timeline-entries.png](crops/timeline-entries.png) |
| Composer 输入面板 | 324–945 | 874–971 | 621 | 98 | 宽 640.8 / 高 101.3 | [composer-panel.png](crops/composer-panel.png) |
| RunStatusBar | 300–1439（跨 Workspace+Inspector，不覆盖左栏账户区） | 984–1023（上边线 984–985，底缘 1021–1023） | 1140 | 内区 986–1020（35；含边线 40） | 高 36.2 / 41.4 | [run-status-bar.png](crops/run-status-bar.png) |
| Inspector 顶层 tab strip | 980–1439 | 0–57 | 460 | 58 | — | [inspector-toptabs.png](crops/inspector-toptabs.png) |
| Inspector 二级 tab（Files/Summary） | 980–1439 | 58–113 | 460 | 56 | — | [inspector-subtabs.png](crops/inspector-subtabs.png) |
| Inspector Files 列表 | 980–1439 | 114–348 | 460 | 235 | — | [inspector-files.png](crops/inspector-files.png) |
| Inspector DiffView 卡片 | 980–1439 | 349–963 | 460 | 615 | — | [inspector-diffcard-head.png](crops/inspector-diffcard-head.png) |
| ActivityPopover | — | — | — | — | State A 为 Inspector 展开态，无 Popover | — |

顶部条带说明：全窗顶部无独立标题栏背景跳变（y=1→2 后整面同底色），traffic lights 直接悬浮于内容视口：红/黄/绿圆点直径 15–16px，中心分别 ≈(25.5,23.5)、(49.5,23.5)、(75,24)，水平间距 24–25.5px，色值见 §4。首个内容文字（Inspector "Changes"）y=30 起，rail 标题 "Pawork" y=59 起。

## 2. 组件量图表

### 2.1 TaskRail（左栏 0–296）

| 组件 | 几何（x/y 为实测范围） | 间距/行高 | 字号（cap 高→估 px） | 字重（目测） | 颜色 | 圆角/描边 |
| --- | --- | --- | --- | --- | --- | --- |
| App 标题 "Pawork" | x23–89，y59–74 | 左边距 23；标题行下缘到下拉框 33 | cap 16→≈22px | medium–semibold（不确定） | #f5f5f4 | — |
| GroupingMenuButton（时钟 glyph+下拉指示） | x220–278，y49–84（59×36） | 右缘 278（右边距 19） | 图标 ~13×13 | — | 边框亮线 ~#1b252e | r≈4（±1），1px 描边 |
| 范围筛选下拉 "All projects" | x20–278，y98–135（259×36） | 距标题行底 23 | 文本 x32–110，cap 12→≈17px | regular | 文本 #d0d1d2（次级亮度） | r≈4（±1），1px 描边 |
| 连接行 "Local · Connected" | 圆点 x25–34（Ø10–11，#6fbf3e），文本 x44–162，y155–167 | 距下拉 20 | cap 12→≈17px | regular | 文本 #c9cacc | — |
| 全局 AddTaskButton "+" | x249–277，y147–174（29×28） | 右缘 277 | 图标 ~12×12 | — | 1px 描边 | r≈2–3（±1） |
| 日期头 "Today"/"Yesterday" | x28–59，y209–221（Today） | Today 距连接行底 42；两桶头间距 380 | cap 13→≈18px | medium（目测） | #a09f9f | — |
| 项目头（Pawork_v2 等 5 个） | 文本 x25–103，如 Pawork_v2 y246–260（含下划线字符） | 项目头→首任务 43–46 | cap 13→≈18px | medium（目测） | #e6e7e8（比任务行亮） | — |
| 项目头 "+" 角标 | x249–277（同全局，29×28） | — | — | — | — | r≈2–3 |
| 任务行（6 行） | 圆点 x18–29（Ø10–11）；标题 x38 起；时间右对齐止于 x272 | 同项目任务行距 43–44；任务→下个项目头 52–54 | 标题 cap 13→≈18px；时间 cap 12→≈17px | regular | 标题 #c9cacc；时间 #858889；运行中蓝点 #235df2；完成绿点 #72c140 | 选中行无背景填充（与未选中同为 #06121a） |
| 账户区 | 分隔线 y936；头像 x19–54（Ø~34）；"Jane Doe" x63–125；chevron x137–147；齿轮 x251–268；内容 y951–987 | 列表底到分隔线间距不定 | cap 13→≈18px | regular | 文本 #cfd0d1 | 头像圆形 |

### 2.2 Workspace Header（300–976，标题行 y56–74）

| 组件 | 几何 | 间距 | 字号 | 字重 | 颜色 | 圆角/描边 |
| --- | --- | --- | --- | --- | --- | --- |
| Task 标题 "Review GUI architecture" | x328–563，y56–74 | 左边距 28（与 Timeline 文字对齐） | cap 17→≈24px | semibold（目测） | #f4f4f4 | — |
| Branch 图标+"main" | 图标 x598–611；文本 x621–651，y60–71 | 标题尾 563→图标 598（35px） | x 字高 9→≈17px | regular | #c2c1c3 | — |
| 状态点+"Completed" | 点 x675–684（Ø~10，#71b13d）；文本 x692–761，y60–71 | — | cap 12→≈17px | regular | #c6c8c7 | — |
| 右侧动作按钮（"+" 圆角方块） | x912–951，y48–84（40×37） | 右缘 951（右边距 25） | 图标 ~16×16 | — | 1px 描边 | r≈3（±1） |

Header 与 Timeline 之间无任何可见分隔线；边界 y≈104 为标题行（底 74）与首条 Timeline 标签（顶 132）空隙中点推断值。

### 2.3 Timeline（内容 y132–843）

| 组件 | 几何 | 间距/行高 | 字号 | 字重 | 颜色 | 圆角/描边 |
| --- | --- | --- | --- | --- | --- | --- |
| 条目标签行（"You"/"Pawork"+时间） | You x328–353，y132–144；时间 x386–446 | 条目间距（标签顶到标签顶）100 | 标签 cap 13→≈18px；时间 cap 12→≈17px | 标签 medium（目测） | 标签 #d8dadb；时间 #808182 | — |
| 消息正文 | x328 起如 "Refine…" y167–182 | 段内行距 ≈24；段落间隙 ≈27 | cap 13→≈18px | regular | #d0d0d0 | — |
| 列表项（• 文本） | 首行 y342–357 | 行距 ≈24（342→366→…→421） | 同正文 | regular | #d0d0d0 | — |
| Tool activity 面板 | x326–944，y491–654（618×164） | 距上文 49 | 面板无标题 | — | 边框 ~#131c24 | r≈5（±1），1px 描边 |
| └ 工具行 ×3 | 行文字带 y512–530/564–583/617–635；图标 x341–360；名称 x375–562；✓ x786–800（Ø14）；"Completed" x810–879；耗时 x905–932 | 行距 ≈54（分隔线 y548–550、y597–600，2px） | 名称 cap 13→≈18px；耗时 cap 12→≈17px | regular | 名称 #d0d0d0；✓ 绿 #45672f–#79bb45 系 | — |
| Run summary 卡片 | x326–944，y667–806（618×140） | 距工具面板 13 | — | — | 边框 ~#131c24 | r≈5（±1） |
| └ 状态圆 ✓+标题 | 圆 x341–380（Ø40）y690–729；"Ready for review" x398–518 | 左边距 15（对齐工具行图标） | 标题 cap 13→≈18px | medium（目测） | 圆 #79bb45；标题 #f0f0f0 | 圆形 |
| └ 说明文字 | 两行 x397–696，y745–766 | 行距 ≈24 | cap 13→≈18px | regular | #8f9091 | — |
| └ "Review changes" 按钮 | x757–924，y690–729（168×40） | 右缘对齐面板右内缘 | 文本 cap 12→≈17px | medium（目测） | 底 #185afd；文字 #ffffff | r≈8–10（±2） |
| └ "Open in editor" 按钮 | x757–924，y748–786（168×39） | 与上按钮间距 19 | 同上 | regular | 1px 描边、透明底 | r≈8–10（±2） |
| Timeline 页脚 | "Run completed · 2m 14s" x326–479；"10:40 AM" x884–943，y829–843 | 距卡片底 23 | cap 12→≈17px | regular | #858889 | — |

### 2.4 Composer（面板 x324–945，y874–971）

| 组件 | 几何 | 间距 | 字号 | 字重 | 颜色 | 圆角/描边 |
| --- | --- | --- | --- | --- | --- | --- |
| 输入面板整体 | x324–945（621 宽），y874–971（98 高） | 左右留白 24/31；与上方 Timeline 空隙 ~1（874 上紧贴） | — | — | 边框 ~#0e181f | r≈4（±1），1px 描边 |
| 输入区（placeholder） | 文本 x338–464，y890–905 | 内左边距 ≈13；输入区高 875–920（46） | cap 13→≈18px | regular | #797a7b（placeholder 暗灰） | — |
| 输入区/控件行分隔线 | y921（1px） | — | — | — | ~#131c24 | — |
| 控件行 | y922–970（49 高） | — | — | — | — | — |
| 模型下拉 "GLM-5.3 · High" | x336–469（133 宽，高 ≈37） | 左内边距 ≈12 | 文本 x349–458，cap 12→≈17px | regular | 文本 #c7c8c9；描边 1px | r≈2–3（±1） |
| 附件按钮（回形针） | 图标 x478–488（按钮 ≈x470–510，~37 高） | 距模型下拉 ~1 | 图标 ~14×14 | — | 图标 #c7c8c9 | r≈2–3 |
| 工作目录下拉 "Pawork_v2" | 图标 x528–538；文本 x572–673；chevron x688–698 | — | cap 12→≈17px | regular | 同上 | r≈2–3 |
| ContextMeter | 文本（Context 78K / 128K）x688–831，y928–937；进度条 x732–821，y948–950 | 距 Send 左缘 ~65 | 标签 cap 10→≈14px | regular | 文本 #9a9b9c；条底 ~#10181f；填充 #205af0（77/90≈61%） | 条高 3，直角 |
| Send 圆钮 | x895–930，y921–956（Ø35–36） | 右内边距 15（面板右缘 945） | 箭头图标 ~16 高 | — | 底为径向渐变：内圈中位 #1a5afc（p10 #1351e5 / p90 #2360fe）；箭头 #e6eefc | 圆形 |
| Composer↔RunStatusBar 空隙 | y972–983（12px 纯背景带） | — | — | — | — | — |

### 2.5 RunStatusBar（x300–1439，y984–1023）

| 组件 | 几何 | 字号 | 字重 | 颜色 | 其他 |
| --- | --- | --- | --- | --- | --- |
| 条体 | 上边线 y984–985（~#192128），内区 y986–1020，底缘 y1021–1023 | — | — | 底 #061219（与 Workspace 同） | 无圆角，通栏 |
| 文本 "Task 92.4K tokens ‖ Z.AI quota 72% left ‖ 38.6 tok/s avg ‖ Run 2m 14s" | 文字带 y1002–1013；词组 x：Task 799–828 / 92.4K 841–880 / tokens 887–930；Z.AI quota… 980–1109；38.6 tok/s avg 1160–1257；Run 2m 14s 1308–1392 | cap 12→≈17px | regular | #848486 | 垂直分隔线 x954–955、x1134–1135、x1282–1283（1–2px）；文字中心 y≈1007.5，较内区几何中心（≈1003）偏下 ~4px |

### 2.6 Inspector（x980–1439）

| 组件 | 几何 | 间距/行高 | 字号 | 字重 | 颜色 | 圆角/描边 |
| --- | --- | --- | --- | --- | --- | --- |
| 顶层 tabs | "Changes" x1011–1072（选中）；"Terminal" x1123–1177；分隔线 x1197–1198；"+" x1217–1229；右侧 chevron x1368–1373、关闭 × x1398–1409 | 左边距 31；strip 底线 y55–57 | 选中 cap 13→≈18px；未选 cap 12→≈17px | 选中 medium（目测） | 选中 #d3d3d6；未选 #87888a | 选中下划线 x993–1090、y56–57（2px，#3578f2） |
| 二级 tabs（Files/Summary） | "Files" x1021–1050；徽标 "4" x1058–1074（Ø15–17）；"Summary" x1118–1179；底线 y112–113 | 距顶层 strip 底 28 | cap 12→≈17px | 选中 medium（目测） | 选中 #d3d3d6；未选 #87888a | 选中下划线 x1001–1094、y112–113（2px，#4172f5） |
| 变更统计行 | "4" x1002–1010；"files" 至 1042；"•" 1050；"+186" 1061–1091；"−24" 1104–1127，y139–150 | 距二级 tabs 底 26 | cap 12→≈17px | regular | 数字 #868383；+186 #619333；−24 #ca6647 | — |
| Files 行 ×4 | 选中行填充 x1001–1413（413 宽），y170–209（40 高），行距 43 | 图标 x1012–1026；路径 x1040–1185；状态 "M" x1390–1401 | 路径 cap 10（等宽）→≈14px | regular | 选中填充 #141d23（未选中无填充）；路径 #c9cacc；M #5c892f | 选中行 r≈6–8（±2） |
| DiffView 卡片 | x980–1439 全宽，y349–963（615 高） | 距 Files 列表底 ~1 | — | — | 边框/分隔 ~#131c24 | r≈12–13（±2） |
| └ 文件头 | 文件名 x1013–1143，y365–381；M x1319–1329；复制 x1352–1365；⋯ x1386–1398 | 头区 350–394；与 hunk 区分隔线 y394–396 | cap 12（等宽）→≈16px | regular | 文件名 #c9cacc；M #5c892f | — |
| └ hunk 头 "@@ …" | x1013–1348，y404–417 | — | cap 12（等宽）→≈16px | regular | 文本 #9a9b9c | 行底 ~#07141c |
| └ diff 行 | 行号沟 x1016–1030；正文 x1045–1359；行距 ≈20–21（431→451→472→493→514…） | — | cap 10（等宽）→≈14px | regular | 上下文 #d0d0d0；删除行底 #231414；新增行底 #0b1f16 | 行底色通栏至卡片右缘 |
| └ 卡片页脚 | "View file" x1021–1076，y938–949；外链图标 x1380–1393 | — | cap 12→≈17px | regular | #c3c5c4 | — |

## 3. 字阶表（像素法）

方法：对文字 crop 做亮度阈值行剖面，量 cap/ascender 高度（含 ±1px 不确定度）；估 px = cap÷0.72（常见 UI sans cap 比例），仅作量级参考。x 字高另注。

| 出现处 | cap 高（px） | 估字号（px） | 备注 |
| --- | --- | --- | --- |
| Workspace Header 标题 | 17 | ≈24 | 全图最大字阶 |
| TaskRail App 标题 "Pawork" | 16 | ≈22 | 次大 |
| 正文/消息/列表/条目标签/项目头/日期头/Inspector tabs/Composer 控件/StatusBar | 12–13 | ≈17–18 | 主体统一字阶；"main"/"Completed" x 字高 9 |
| Inspector 等宽（文件路径、diff 正文） | 10 | ≈14 | 等宽字体 |
| hunk 头 / diff 文件名（等宽） | 12 | ≈16 | 与正文差在不确定度边缘，未确证独立字阶 |
| ContextMeter 标签 | 10 | ≈14 | 小字阶 |
| 时间戳（rail/条目/页脚） | 12 | ≈17 | 与主体同级，用颜色区分层级 |

## 4. 颜色取样表（平坦区内圈中位；token 列为 R1 前生产值）

| 位置 | 实测 hex | 对照 token | 偏差 |
| --- | --- | --- | --- |
| TaskRail 底 | #06121a | sidebar #161616 | 显著更暗且偏蓝（ΔL 大） |
| Workspace 底 | #061219 | bg #1e1e1e | 显著更暗且偏蓝 |
| Inspector 底 | #06121a | bg #1e1e1e | 同上 |
| Rail\|Workspace 分隔线 | #1c242b | border #2e2e2e | 更暗偏蓝 |
| Workspace\|Inspector 分隔线 | #192128 | border #2e2e2e | 更暗偏蓝 |
| 标题文字（Header/rail 标题） | #f4f4f4 / #f5f5f4 | text.primary #e8e8e8 | 更亮 |
| 正文/列表标题 | #d0d0d0 / #c9cacc | text.primary #e8e8e8 | 更暗（介于 primary/secondary 之间） |
| 次要文字（时间/StatusBar/未选 tab） | #858889 / #808182 / #848486 / #87888a | text.secondary #9a9a9a | 略暗 |
| 选中任务蓝点 | #235df2 | accent.primary #2f6fed | 更饱和偏亮 |
| Send 圆钮 | #1a5afc（径向渐变 #1351e5–#2360fe） | accent.primary #2f6fed | 更饱和 |
| Tab 下划线（顶层/二级） | #3578f2 / #4172f5 | accent.primary #2f6fed | 接近但更亮 |
| ContextMeter 填充 | #205af0 | accent.primary #2f6fed | 接近 |
| "Review changes" 按钮 | #185afd | accent.primary #2f6fed | 更亮偏蓝 |
| 完成绿点（rail/Header） | #72c140 / #71b13d | semantic.success_bg #3d7a4a | 明显更亮、偏黄绿 |
| 摘要卡 ✓ 圆 | #79bb45 | semantic.success_bg #3d7a4a | 同上 |
| +186 / 文件 M | #619333 / #5c892f | semantic.success_bg #3d7a4a | 更暗 |
| −24 | #ca6647 | semantic.danger_bg #8a3b32 | 更亮偏橙 |
| diff 删除行底 | #231414 | semantic.danger_bg #8a3b32 | 明显更暗（低透明度叠加观感） |
| diff 新增行底 | #0b1f16 | semantic.success_bg #3d7a4a | 明显更暗 |
| Files 选中行填充 | #141d23 | surface.raised #2a2a2a | 更暗偏蓝 |
| Composer 面板底 | #0a151c | — | 介于 bg 与 raised 之间，偏蓝 |
| Traffic lights（红/黄/绿） | #fd7562 / #fcb057 / #6ea648 | — | macOS 系统控件 |

结论：v3 图整体底色/边框比 R1 前生产 token 更暗且带蓝调，绿系更亮黄，蓝系更饱和。本表是 2026-08-26 色板仲裁的取样证据；新的冻结目标以 [design/README.md §2.1](../../../design/README.md) 为准，渐变/抗锯齿仍不单独建 token。

## 5. 8px 基线核对与冲突表

### 5.1 8px 基线核对（归一值 → 原画布换算）

| 量 | 归一（px） | 原画布（px） | 8px 网格判定 |
| --- | --- | --- | --- |
| Workspace 文字左边距 | 28 | 28.9 | ≈4×8−3，近 24/32 之间 |
| Composer 面板左右留白 | 24 / 31 | 24.8 / 32.0 | 右侧吻合 32，左侧近 24 |
| Composer 面板高 | 98 | 101.3 | 非 8 倍数（≈96+5） |
| 工具面板/摘要卡左右留白 | 26 / 30–31 | 26.8 / 31–32 | 右侧吻合，左侧偏 3 |
| Files 行距 | 43 | 44.4 | 非 8 倍数（≈5.5×8） |
| 工具行距 | 54 | 55.7 | ≈7×8=56，差 0.3 |
| Timeline 正文行距 | 24 | 24.8 | ≈3×8=24，差 0.8 |
| diff 行距 | 20–21 | 20.7–21.7 | 非 8 倍数 |
| Inspector tabs 左边距 | 31 | 32.0 | 吻合 32 |
| Header 右按钮右边距 | 25 | 25.8 | ≈24+2 |
| 任务行距（rail） | 43–44 | 44.4–45.4 | ≈5.5×8 |

结论：定稿图本身并非严格 8px 网格（AI 出图特性）；多数值落在 8±3px 邻域或 4px 半格上。2026-08-26 已裁定实现继续遵循 design §2 的 8px 节奏与明确组件合同，本表数值只作视觉参考，不反向改写合同。

### 5.2 与 design/README.md §2 定稿值冲突表

| 项 | §2 定稿值（原画布） | 实测（归一→原画布） | 差值 | 判定 |
| --- | --- | --- | --- | --- |
| TaskRail 宽 | 288 | 297→306.5 | +18.5（+6.4%） | 图像冲突；已裁定实现保持 288 |
| Inspector 宽 | ~440 | 460→474.7 | +34.7（+7.9%） | 图像冲突；已裁定实现保持 ~440 |
| Composer 常态总高 | 88–94 | 98→101.3 | +7~13 | 图像冲突；已裁定实现保持 88–94 |
| 底部控件高 | 28–30 | 模型下拉 37→38.5 | +8.5~10.5 | 图像冲突；已裁定实现保持 28–30 |
| Send 尺寸 | 32 | Ø35–36→36.2–37.2 | +4~5 | 图像冲突；已裁定实现保持 32 |
| RunStatusBar 高 | 24 | 内区 35→36.2（含边线 40→41.4） | +12~17 | 图像冲突；已裁定实现保持 24 |
| ActivityPopover ~320 | — | State A 无 Popover | — | N/A（State B 量图核对） |
| 8px 间距基线 | 全局 | 见 §5.1 | — | 图像部分冲突；已裁定实现继续按合同节奏 |
| 选中态背景（§3.6 要求「背景+焦点+名称」） | 应有背景 | rail 选中任务行无填充（仅蓝点） | — | 图像冲突；实现仍按 §3.6 执行背景选中态 |
| 色板 | §2 token 表 | 见 §4 | — | 已按三态取样与对比度重定，目标见 design §2.1 |

## 6. 方法与不确定度说明

- 工具：Pillow 12.3.0 + numpy 2.3.5（brief 指定 bundled python）；脚本位于 /tmp/r1a-state-a/，未入仓库。
- 区域边界：行/列中位数颜色剖面定位 surface 跳变与分隔线（阈值法，±1px；2–3px 线宽按亮带整体记录）。
- 字号：文字 crop 亮度阈值行剖面量 cap/ascender/x 字高（±1px）；估 px=cap÷0.72，不确定度 ±1–2px；字重为目测（regular/medium/semibold），不确定处已标 unknown 性质说明。
- 颜色：平坦区内圈像素中位数（抗 AA），hex 保留 8bit；渐变/AA 区不取样。
- 圆角：顶边平直段与包围盒宽度差 ÷2 估计（±1px；卡片类 ±2px）。
- 换算：归一→原画布 x÷0.969044、y÷0.96695（normalization-report.json State A）。
- 已知测不出/推断项：Header/Timeline 分界无可见线（取空隙中点推断 y≈104）；hunk 头与 diff 正文是否不同字阶（差异在不确定度内，未确证）；Composer 控件行内纸夹按钮独立边界与下拉框边框颜色接近背景，bbox 取图标外扩估计（±2px）。
- 诚实声明：所有数值均为本图实测；rail 选中行「无背景填充」、StatusBar 文字垂直偏下 ~4px、图面色板与 token 偏差均为实测事实，非推断。
