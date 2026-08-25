# State B 量图（Timeline · Inspector 折叠 · ActivityPopover）

对象：[reference.png](reference.png)，1440×1024；源图为无 ICC 的 RGB，按 sRGB 解释。坐标采用左上原点；表中 x/y/w/h 表示半开区间 [x,x+w)×[y,y+h)，因此边界像素最后一位有 ±1 px 不确定度。证据为 crops/ 下同名 1:1 PNG。

## 1. 区域几何表

| 区域 | x | y | w | h | 证据 |
| --- | ---: | ---: | ---: | ---: | --- |
| macOS titlebar 安全条 | 0 | 0 | 1440 | 48 | window-chrome.png；红/黄/绿灯均为 x=18/43/68、y=17、直径 16–17，中心距 25–26 |
| TaskRail | 0 | 0 | 320 | 1024 | taskrail.png；右分隔线中心 x≈320 |
| Workspace（Inspector 折叠后扩展） | 320 | 0 | 1120 | 1024 | taskrail.png 右缘 + statusbar.png；无 Inspector 列/分隔线 |
| Workspace Header | 320 | 48 | 1120 | 49 | workspace-header.png；底边过渡 y=92–96 |
| Timeline / Workspace 内容区 | 320 | 97 | 1120 | 781 | timeline.png；可读列实际为 x=347..962 |
| ActivityPopover（浮层） | 1063 | 94 | 352 | 347 | activity-popover.png |
| Composer | 347 | 878 | 616 | 97 | composer.png；上方 y=830..876 是 Run 摘要卡，不并入 Composer |
| StatusBar | 320 | 982 | 1120 | 42 | statusbar.png；文字 y=999..1013 |
| Inspector（折叠） | — | — | 0 | — | state-b 结构：右列与分隔线均不可见 |

Workspace Header 右上两个动作槽位：

| 槽位 | x | y | w | h | 中心 | 说明 |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| 左动作（方形加号 glyph；像素图不能确认语义） | 1328 | 48 | 38 | 39 | (1347,67.5) | header-actions.png |
| 右动作 / Activity 触发器 | 1376 | 48 | 39 | 39 | (1395.5,67.5) | 由下方锚点与 Popover 右对齐确认 |

两槽位中心距 48 px，外缘间隙约 10 px；右槽右缘 x≈1414，距 Workspace 右缘 25 px。标题同排还可见 branch 图标 + `main` 与绿色状态点 + `Completed`；此前把 x=351..784 整段误记为标题，2026-08-26 复审已按 [workspace-header.png](crops/workspace-header.png) 拆分纠正。

### ActivityPopover / Composer 不覆盖证明

- Popover 外框：[1063,1415)×[94,441)。
- Composer 外框：[347,963)×[878,975)。
- 横向间隙：1063−963 = 100 px；纵向间隙：878−441 = 437 px。
- 两个轴向间隙均 >0，因此 bounding box 不相交；即便只看纵向也相隔 437 px。Popover 同时不横穿 Timeline 可读列（右缘 x=962）。

## 2. 组件量图表

### TaskRail

| 组件 | 几何 / 间距 | 文本与图标 | 颜色 / 形状 |
| --- | --- | --- | --- |
| Pawork 标题行 | 内容 y=52..88；标题 x=23..82、y=61..79；GroupingMenuButton x=239..299、y=52..89 | Pawork cap≈16、x-height≈12；按钮内 clock glyph+chevron | 标题亮灰；按钮描边约 #21292e，圆角约 8px±1 |
| Scope 筛选按钮 | x=20..294、y=100..145 | “All projects” cap≈15、x-height≈10 | ghost/raised 边界低对比，圆角约 8px±1 |
| 连接状态 + AddTask | 内容 y=162..175；AddTask 外框 x=268..300、y=154..183 | “Local · Connected” cap≈13、x-height≈10 | 状态点实测 #71c141；加号槽约 32×29 |
| 日期/项目/Task 列表 | Today 标签 x=18..61、y=216..233；项目行 y=254/348/488/627/722；Task 行 y=297/390/432/532/669/766 | 项目/Task cap≈13、x-height≈10；Task 行距 42–53 px | 项目行右侧 + 槽 x=276..290、约 14×14 glyph；状态点保留 |
| 账户 / 设置 | 分隔线 y=940..942；头像 x=18..56、y=954..993；设置图标 x=270..291、y=963..985 | “Jane Doe” cap≈13 | 头像圆形直径约 38；分隔线约 #232b31 |

### Workspace Header 与 ActivityPopover

| 组件 | 几何 | 文本 / 行高 | 颜色 / 形状 |
| --- | --- | --- | --- |
| Header 标题 | x=351..586、y=60..78 | cap≈18、x-height≈12；单行 | 文字高亮像素中位 #c5cac9 |
| Header branch | 图标 x≈618..632；`main` x≈640..672；y=60..78 | cap≈12；与标题同排 | 图标与文字灰白 |
| Header 终态 | 绿色点 x≈694..706；`Completed` x≈714..786；y=60..78 | cap≈12；点 + 文字双编码 | 状态点绿色，文字灰白 |
| Activity 触发器 | x=1376..1415、y=48..87 | 38×39 槽位 | glyph #e4e6e7；边框约 #21292e |
| 锚点指针 | x=1387..1403、y=87..95 | 16×8 | 指针中心 x≈1395，正对触发器中心；Popover 顶边 y=94 |
| Popover 外框 | x=1063..1415、y=94..441 | w=352、h=347 | 底 #0e171d；右边界 #2c3238、顶边界 #252a2f；描边约 2px，圆角 10–12px±1 |
| Popover header | 标题 x=1083..1140、y=116..133；外部链接图标 x=1375..1393；分隔线 y=146..150 | Activity cap≈14、x-height≈10；header 高约 54 | 内左/右 padding 20/22 |
| Changes 分区 | 标签 x=1084..1142、y=172..186；摘要值 x=1084..1220、y=207..219 | 标签 cap≈12；摘要行 cap/x≈12 | 绿色数字 #558b2e，红色数字 #8a3c28 |
| Agents 分区 | 标签 x=1083..1130、y=257..270；三条 Agent 行 y=290..313、342..365、394..417 | 行内容高 23 px，行中心距 52 px | 行内状态图标不被摘要遮蔽 |
| 分区间距 | divider→Changes 25；Changes→摘要 22；摘要→Agents 39；Agents→首行 21；末行→内底 23 | 全部 px | 20/21/22/23/25/39/52 混合，非严格 8px 倍数 |

### Timeline

| 组件 | 几何 / 行高 | 颜色 / 形状 |
| --- | --- | --- |
| 可读列 | x=347..963；消息/卡片均共享该 616 px 宽 | 卡片边框约 #151d23，圆角约 8–10px±1 |
| 消息节奏 | sender1 y=139..152、正文 y=174..190；sender2 y=242..255、正文 y=281..297；正文段落到列表行距 32–33 px | sender 名与时间同线；时间在 x=407..469 / 435..496 |
| Assistant 列表 | 标题/列表行 y=324..338、355..371、380..397、412..430、439..455、473..487 | 行距 32–33 px；正文亮/次亮层级可分辨 |
| Tool activity 卡 | x=347..963、y=508..690；三行 y=531..550、583..603、638..657 | 行中心距 53/54.5 px；分隔线 y=566..569、617..621、675..677 |
| Tool 行内部 | 左图标 x≈363..382；标题/状态在 x≈398..900；时长 x≈925..951 | 状态图标与容器保留 |
| 完成摘要卡 | x=347..963、y=705..812 | 成功图标 x=363..403、y=711..754；主按钮 x=774..944、y=708..753；次按钮 x=774..944、y=763..810 |
| 完成卡按钮 | 主/次按钮均 170×45/47，纵向间隙约 10 px | 主按钮蓝 #1f5bfd；次按钮 ghost/outline |
| Run 摘要卡 | x=347..963、y=830..877；文字 y=849..861 | 与 Composer 顶边 y=878 相邻但不重叠 |

### Composer 与 StatusBar

| 组件 | 几何 / 间距 | 文本 / 图标 | 颜色 / 形状 |
| --- | --- | --- | --- |
| Composer 外框 | x=347..963、y=878..975；输入区到 footer 分界 y≈925 | 常态总高 97 px | 底 #0e171d；顶边 #181f24、底边 #0c151c；描边约 2px，圆角 8–10px±1 |
| Placeholder | x=360..485、y=893..907 | cap≈13、x-height≈9 | #636566 |
| Footer 控件带 | y=925..962；左右 padding 12/0 | 控件高约 37 px | 背景 ghost，表面与卡面对比 <2 灰阶，槽位以边缘/内容定位 |
| Model selector | x=359..524、y=925..963 | 值文本 x=370..480、y=937..954，cap≈13 | 文字 #a5a7a8 |
| Attachment | x=521..585、y=925..963 | paperclip glyph 13×21 | 槽宽约 64 |
| Workspace selector | x=581..731、y=925..963 | 值文本 x=620..692 | folder+chevron 保留 |
| ContextMeter | x=751..888、y=925..963；文本 y=930..942，bar x=751..887、y=950..957 | label/value cap≈12；bar 高 6 | 填充 #2565f3，轨道 #2e3336 |
| Send | x=914..951、y=925..962 | 37×37 圆形 | 填充 #225dfd，白色向上箭头 |
| StatusBar | x=320..1440、y=982..1024；文本 x=827..1440、y=999..1014；分隔符 x=981/1157/1300 | cap≈12、x-height≈11；单行 | 文字 #737374；可见内容右置，而非从 Workspace 左缘开始 |

## 3. 字阶表（像素法）

方法：对文字 bbox 做 max-channel 阈值行剖面；cap/ascender 顶到基线为 cap height，高像素计数平台为 x-height。所有尺寸 ±1 px；字重为目测。

| 出现处 | y bbox | cap/ascender | x-height | 行高 / pitch | 字重目测 |
| --- | ---: | ---: | ---: | ---: | --- |
| TaskRail Pawork 标题 | 61..79 | 16 | 12 | 单行 | medium/semibold |
| TaskRail scope / 连接 / 列表行 | 116..131、162..175、254..270、297..310 | 13–15 | 9–10 | 列表 42–53 | regular |
| Workspace Header 标题 | 60..78 | 18 | 12 | 单行 | medium/semibold |
| Timeline sender / time | 139..152、242..255 | 13 | 10 | 103 | regular；sender略重 |
| Timeline 正文 / 列表 | 174..190、281..297、324..487 | 13 | 9 | 32–33 | regular |
| Tool 行标题 / 状态 / 时长 | 531..549、583..603、638..657 | 14 | 10 | 53–54 | regular；状态略轻 |
| 完成摘要标题 / 描述 / 按钮 | 725..742、768..806 | 13–14 | 10 | 描述约 28 | 标题 medium；正文 regular |
| Popover Activity 标题 | 116..133 | 14 | 10 | header 54 | medium/semibold |
| Popover 分区标签 / 摘要 / Agent 行 | 172..186、207..219、290..313 | 12–14 | 9–10 | Agent 52 | 标签 regular；Agent 名 medium |
| Composer placeholder / footer 值 | 893..907、930..954 | 12–13 | 9 | 单行 | regular |
| StatusBar | 999..1014 | 12 | 11 | 单行 | regular |

未把 cap height 换算成 point/px 字号：图中字体家族与 em metrics 未知，换算只能引入额外假设。

## 4. 颜色取样表（token 列为 R1 前生产值）

取样：平坦背景取区域内圈中位；文字取高亮度像素中位；彩色状态用色相掩膜；边框取单像素线段中位。sRGB hex，AA/渐变混入时已在用途列说明。

| 用途 | 实测 | token 表 | 偏差 / 结论 |
| --- | --- | --- | --- |
| Workspace bg | #09131b | bg #1e1e1e | 明显更深且带蓝相 |
| TaskRail bg | #0a131c | sidebar #161616 | 明显更深且带蓝相 |
| TaskRail 分隔线 | #232b31 | border #2e2e2e | 偏暗约 5–11 |
| Popover / Composer / raised 面 | #0e171d | surface.raised #2a2a2a | 明显更深 |
| Popover 右 / 顶描边 | #2c3238 / #252a2f | border #2e2e2e | 接近，带 AA/渐变 |
| Header 标题 | #c5cac9 | text.primary #e8e8e8 | 偏暗约 20–25 |
| 次要文字（Popover/Timeline/Status） | #787979 / #7e7f7f / #737374 | text.secondary #9a9a9a | 偏暗约 28–39 |
| 活动任务点 / Context fill / Send / 主按钮 | #2962f6 / #2565f3 / #225dfd / #1f5bfd | accent.primary #2f6fed | 蓝通道 +14..16、红绿更低；同族但非精确 |
| Changes 绿 / 红 | #558b2e / #8a3c28 | success_bg #3d7a4a / danger_bg #8a3b32 | 红接近；绿更黄 |
| TaskRail 成功点 | #71c141 | success_bg #3d7a4a | 角色可能不同（状态点 vs 背景），不能直接判 token 失败 |
| Context track | #2e3336 | border #2e2e2e | 接近 |

hover / active / focus 色在静态 State B 不可见，不虚构取样。

本表是 2026-08-26 色板仲裁的取样证据；新的冻结目标以 [design/README.md §2.1](../../../design/README.md) 为准。

## 5. 8px 基线核对与冲突表

### 8px 核对

通过项：TaskRail 320、Workspace 1120、动作槽中心距 48、Popover/Agent 行距 52、Composer 宽 616、Tool 行距约 53（1px 内）。

不通过或不判定项：Header 高 49、Timeline 高 781、Composer 高 97、StatusBar 高 42、Popover 高 347、Popover 内 padding/间距 20/21/22/23/25/39、Rail 行距 42–53 混合。结论：不能宣称整图严格 8px 基线；只能说主栅格存在 8px 倍数，细节存在半步/任意值。

| 冲突 ID | design/README.md 定稿值 | State B 实测 | 结论 |
| --- | --- | --- | --- |
| C-01 | TaskRail 288 px（§2） | 320 px | +32 px，超出 §0.1 主栏 ±8 px/±1.5% |
| C-02 | Composer 88–94 px（§2） | 97 px | +3 px 超上限；仍在 §0.1 主几何 ±8 内，但不符合声明范围 |
| C-03 | RunStatusBar 24 px（§2） | 42 px | +18 px |
| C-04 | ActivityPopover 约 320 px（§2/§5.1） | 352×347 px | 宽 +32 px；右锚点/零遮挡行为本身符合 §5.1 |
| C-05 | §6/UI Review F-05 期望 Header 含 branch 与终态 | 图上同排具备 branch 图标 + `main`、绿色点 + `Completed` | PASS；此前整段误记为标题，复审已纠正 |
| C-06 | R1 前生产 token 色板 | 主要底/文字/raised 均偏深偏蓝 | 已按三态取样与对比度仲裁重定，目标见 design/README.md §2.1 |
| C-07 | §2 8px 基线 | 多个主值非 8 倍数 | 基线执行混合，不可判 PASS |

## 6. 方法与不确定度

- 区域边界：对指定横/纵窗口计算相邻行列 RGB 差的持久边，只取跨越长距离的边；Popover/Composer 再用角点与线段复核。边界 ±1 px。
- 组件槽位：阈值/色相连通域给出 glyph bbox，控件表面再以局部低对比边复核。低对比 ghost 表面 ±2 px。
- 字阶：文字 bbox 行剖面；cap/x-height ±1 px，字重只目测，unknown 不写成事实。
- 颜色：平坦区中位；文字/彩色使用明确亮度或色相掩膜。渐变、AA 和原图归一可能带来 1–3 灰阶漂移，但不能解释第 4 节的大幅底色偏差。
- 圆角：由角弧边界进入直线段的偏移估计；Popover 10–12 px，卡片 8–10 px，均 ±1 px。
- 交互：静态 reference 不能证明 hover/active/focus/键盘/滚轮行为，全部不推断。
