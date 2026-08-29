# Pawork V2 Desktop GUI 视觉实施基准

> 状态：已选定，v3 修订（2026-08-17）
>
> 设计方向：方案 2，三栏 Coding Agent 工作台
>
> 需求事实源：[GUI 设计](../docs/gui-design.md)
>
> 用途：后续 `apps/desktop` GUI 实现、审查与视觉验收的默认参照

本目录冻结 Pawork V2 Desktop 的目标视觉与关键交互。后续实现应先对照这里的界面状态，再按 [GUI 设计](../docs/gui-design.md) 的阶段范围接入真实能力；截图中提前出现的后续阶段 Surface 不会因此提前进入当前阶段。

若发生冲突，优先级为：架构红线与协议契约 → 当前阶段任务书 → `docs/gui-design.md` 行为规则 → 本目录的视觉细节。

## 1. 定稿视图

| 视图 | 用途 | 当前资产 |
| --- | --- | --- |
| Timeline（Inspector 展开） | 日期内再按项目组织 Task；项目头可定向新建 | [desktop-shell-timeline-v3.png](desktop-shell-timeline-v3.png) |
| Timeline（Inspector 折叠） | Workspace 扩展，右上保留 Changes 与 Agent activity 浮窗 | [desktop-shell-timeline-collapsed-v3.png](desktop-shell-timeline-collapsed-v3.png) |
| Projects（按项目） | 按项目展开、折叠并从项目头定向新建 Task | [desktop-shell-projects-v3.png](desktop-shell-projects-v3.png) |

### Timeline

![Pawork Desktop Timeline 视图](desktop-shell-timeline-v3.png)

### Timeline · Inspector 折叠

![Pawork Desktop Timeline Inspector 折叠视图](desktop-shell-timeline-collapsed-v3.png)

### Projects

![Pawork Desktop Projects 视图](desktop-shell-projects-v3.png)

本目录仅保留本节列出的 v3 定稿资产；评审过程中的历史版本已删除，不作为实现目标。

## 2. 壳层与布局

- 定稿图画布：约 `1486 × 1058`（ImageGen 输出可能有 `1 px` 边差）；实现仍须在 `1440 × 1024` 对照验收，当前默认窗口 `1080 × 720` 必须可用。
- 宽屏采用三栏：左侧 `TaskRail` 288 px、中央 `WorkspaceView` 弹性伸缩、右侧 `InspectorPanel` 约 440 px。
- `1080–1279` 宽度下左栏收敛到 240 px，右侧 Inspector 默认折叠为抽屉；中央对话区不得小于 560 px。
- `Composer` 默认高 88–94 px；底部控件高 28–30 px，Send 为 32 px。多行输入按需向上增长，不把常态输入框做成工具栏容器。
- `RunStatusBar` 高 24 px，位于 Workspace 与 Inspector 底部，不覆盖左栏账户区；Composer 始终位于它上方。
- Inspector 展开时约 440 px；折叠时宽度归零并让 Workspace 扩展，右上以约 320 px 的 `ActivityPopover` 保留轻量态势，不挤压 Composer、ContextMeter 或审批主操作。
- 采用 8 px 间距基线；列表、工具活动与 diff 保持原生桌面密度，不使用仪表盘卡片墙。三图实测存在系统性偏离 8px 网格的细节（见 §2.1），实现以本节几何定稿值为准。
- 色板以 §2.1 的 2026-08-26 重定 token 为准（用户拍板，取代旧的「当前实现色板」基线）。设计图中的细微抗锯齿或渐变不应被硬编码成新 token（Send 等按钮的径向渐变取实体主色）。

### 2.1 几何合同与色板（2026-08-26 R1 Wave A 用户拍板）

**几何合同**：本节 §2 的定稿值（TaskRail 288、Inspector ~440、Composer 88–94、StatusBar 24、ActivityPopover ~320）是实现合同。三张定稿图是近似视觉语言参考，不是逐像素几何事实源——R1 量图实测三图互不一致（TaskRail 283/297/320、Composer 97–102、StatusBar 35–46，明细见 [docs/ui-review/state-*/measurements.md](../docs/ui-review/README.md) 冲突表）。图像与合同的偏差优先由 SSIM 分区的 reference/current 矩形、左右锚点和最低共同覆盖率表达（[docs/ui-review/README.md](../docs/ui-review/README.md) §3）；`geometry-drift` 遮罩只允许收敛到锚点仍无法表达的纯边缘背景，当前三态均不使用。结构与几何硬门禁按合同值 + [UI_Review §0.1](../docs/UI_Review.md) 容差判定。字阶与组件细节以三份量图表为准（在 §0.1 容差内取实测档）。

**色板（新冻结 token 目标，R2 落 theme.rs）**：下表「实测」值取自 R1 量图（1440×1024 坐标，方法见各 measurements.md §6）；「派生」值为主代理按 token 语义内插，R2 在门禁下微调。

| token | 旧值 | 新目标 | 依据 |
| --- | --- | --- | --- |
| bg.base | #1e1e1e | #07121a | State A workspace 空区实测 |
| bg.panel | #161616 | #061219 | State A rail / StatusBar 实测 |
| bg.menu | #1a1a1a | #0e171d | State B ActivityPopover 实测 |
| surface.raised | #2a2a2a | #10171c | State C 选中行实测（State A Files 选中 #141d23，范围记录） |
| surface.hover | #343434 | #182229 | 派生（raised 上浮约 8 灰阶） |
| surface.disabled | #242424 | #0c161c | 派生 |
| border.subtle | #2e2e2e | #1a2129 | State C 分隔线实测范围 #1a1f26–#282b33 折中 |
| border.strong | #3a3a3a | #2c3338 | State B Popover 描边实测 |
| text.primary | #e8e8e8 | #f0efec | State A 标题 #f5f5f4 / State C #dddcd8 折中 |
| text.emphasis | #c8c8c8 | #d0d0d0 | State A 正文实测 |
| text.secondary | #9a9a9a | #8a8d8c | State A 时间 #858889 / State C 次级 #81817f |
| text.tertiary | #7f7f7f | #7f7f7f | 实测下界不满足小字对比度；保留可访问档 |
| text.placeholder | 白 30% | #7f7f7f（不透明） | 避免透明色叠在新深色 surface 后低于小字对比度门槛 |
| text.assistant | #d7d7ff | 收敛到 text.emphasis | 三图 assistant 正文与用户正文同色，无蓝紫分化 |
| text.tool | #9cdcfe | 收敛到 text.secondary | 图上 tool 名 #959292（State C），无蓝色分化 |
| accent.primary | #2f6fed | 保持 | 实测蓝区间 #235df2–#4172f5 含现值；Send 径向渐变 #1351e5–#2360fe 取实体色 |
| accent.hover | #3d7bf0 | #3270e8 | 派生；保留蓝色上浮感并满足白字对比度 |
| semantic 状态点绿 | —（success_bg #3d7a4a 保持） | 状态点绿 #74c94c（候选 semantic.success_fg，R2 定槽位） | 三图状态点实测 #71b13d–#7bcf4d |
| semantic.success_hover | #4a8c58 | #438251 | 派生；满足白字对比度 |
| danger / warning 系 | 保持 | 保持 | 量图未见显著偏离（State B C-06） |

可访问性约束按“文字角色 × 允许 surface”组合判定，不把单个 token 宣称为可与任意背景互换：`text.secondary #8a8d8c` 可落在 `surface.hover #182229`，对比度约 4.82:1；`text.tertiary` / `text.placeholder #7f7f7f` 最亮只允许落到 `surface.raised #10171c`，约 4.52:1，不得用于更亮的 hover surface（会降到约 4.04:1）。白字对 `accent.hover #3270e8` 与 `semantic.success_hover #438251` 约为 4.55:1 / 4.61:1。R2 落 token 后须按真实组件组合重新跑定向对比度门禁；新增组合不能借用其它背景的通过值。

## 3. 左侧 TaskRail 紧凑操作

左栏从上到下固定为：Pawork 标题与 `GroupingMenuButton` → 项目范围筛选 → 连接状态与全局 `AddTaskButton` → 可滚动日期 / 项目 / Task 列表 → 账户与设置。分组与新建均使用带 tooltip 和可访问名称的角标按钮，不再占用整行。

### 3.1 范围筛选与分组正交

- 顶部范围筛选默认是 `All projects`，也可限定到一个项目。
- 范围筛选决定“哪些任务可见”；`Timeline / Projects` 只决定“同一批任务如何组织”，二者不能复用成同一个控件。
- 项目身份使用 Session 的 canonical `workspace_id` 与 Workspace 元数据；不得从任务标题或任意绝对路径猜测。
- 选择具体项目后，Timeline 仍按时间分桶；Projects 只显示该项目组。回到 `All projects` 后恢复各项目展开状态。

### 3.2 GroupingMenuButton

- 位于 Pawork 标题行右侧，替代宽幅 segmented control；Timeline 使用 clock/list glyph，Projects 使用 folder/list glyph，并带小型下拉指示。
- 点击打开只有 Timeline 与 Projects 的轻量菜单，当前项带 checkmark；关闭状态不显示文字标签或常驻 popover。
- tooltip 与 accessible name 必须明确当前模式，例如 `Group tasks · Timeline`，不能只靠 glyph 区分。

### 3.3 AddTaskButton 与 ProjectAddTaskButton

- `Local · Connected` 右侧保留全局 `AddTaskButton`；`All projects` 下创建后必须在 Composer 中确认工作目录，单项目范围下默认继承该 Workspace。
- Timeline 的每个日期桶内项目头、Projects 的每个项目头均显示 `ProjectAddTaskButton` 加号角标；新 Task 默认绑定该项目的 canonical `workspace_id`，不能从标题或路径字符串猜测。
- 两类按钮均不使用全宽样式；断线或 projection stale 时禁用并提供原因，可用时保留 tooltip、键盘焦点与快捷键入口。

### 3.4 Timeline（按时间）

- 层级固定为日期 → 项目 → Task；分桶顺序为 Today → Yesterday → Previous 7 days → Earlier。
- 每个日期桶只显示当日有 Task 的项目；项目按最近活动时间排序，Task 在项目内按最近活动时间倒序，项目头右侧提供定向新建角标。
- Task 行显示运行状态、标题与最近活动时间；项目身份由上级项目头表达，不在每行重复项目名。
- 同一 Session 只出现一次；时间变化只移动现有行，不复制任务。

### 3.5 Projects（按项目）

- 项目按最近活动时间排序；项目头显示名称、展开状态、当前范围内任务数量与定向新建角标。
- 项目内任务按最近活动时间倒序；没有 Workspace 元数据的历史 Session 进入 `Unassigned`，不能静默丢失。
- 展开/折叠只改变呈现，不影响任务运行与当前会话。

### 3.6 切换不变量

- 切换分组方式不改变 active session、Composer 草稿、滚动中的 Timeline 或正在运行的 Run。
- 当前任务在新视图中自动滚动到可见位置；选中态必须同时具有背景、焦点与可访问名称，不能只靠状态点颜色。
- 分组方式、项目展开状态和范围筛选属于本地 GUI presentation preference，可本地持久化；它们不是 Agent domain 事件，也不通过 Provider 特例实现。
- 键盘焦点顺序为范围筛选 → GroupingMenuButton → 全局 AddTaskButton → 项目头 / ProjectAddTaskButton → Task；菜单内上下键移动、`Enter` 选择、`Escape` 关闭。

## 4. ContextMeter 与 RunStatusBar

### 4.1 ContextMeter

- 位于 Composer 工作目录选择器与 Send 之间；宽屏显示 `Context 78K / 128K` 与细进度条，窄屏可收敛为 `61%`，但不能挤掉 Send。
- 分子来自当前请求组装后的权威 token estimate，分母来自 model catalog 的 context window；不能用任务累计 token 代替上下文占用。
- 未知 context window 显示 `Context unavailable`，不编造容量。超过 soft limit 变为警告色，接近硬上限时显示明确文本，不只换颜色。

### 4.2 RunStatusBar

宽屏按固定优先级展示，数据更新不得引起布局跳动：

1. `Task <n> tokens`：当前 Session 累计 input + output；cache read/write 只在详情中展开，避免重复计数。
2. `<provider> quota <remaining>`：只显示 quota-service 的权威剩余额度；`Unknown` 显示 `Quota unavailable`，不得伪造 0 或百分比。
3. `<n> tok/s`：流式期间显示实时 output tokens/s；Run 终态显示并标注 `avg`，无可用时间戳时显示 `—`。
4. `Run <duration>`：由权威 Run 起止时间计算；运行中实时更新，时间戳不完整时显示 `—`。

模型与 reasoning 只在 Composer 的模型选择器显示，并由 Core 确认状态覆盖本地 pending；`RunStatusBar` 不重复。窄屏保留 Task tokens，余项收进可键盘访问的 status details popover；ContextMeter 仍留在 Composer，状态栏不承载主操作。

## 5. InspectorToolTabs

- Inspector 顶层使用可扩展 tab strip：Changes、Terminal、Add tool；Files / Summary 是 Changes 内部二级 tab，不能与顶层混用。
- 顶层 tab 只由 Host capability 与当前阶段启用：Changes 随 S8，Terminal 随 S10；未接通时隐藏或明确 disabled，不做可点击假入口。
- Add tool 只管理已注册的 Inspector surface，不直接访问 Provider、数据库、Git 或工具；所有数据仍经 controller → `pawork-client`。
- 切换 tab 不改变 active session；每个工具保留独立滚动与展开状态，关闭 Inspector 后可恢复。

### 5.1 折叠态 ActivityPopover

- 折叠 Inspector 后移除整列与分隔线，Workspace 使用释放出的宽度；Activity 触发器固定在 Workspace Header 右上（折叠态常驻角标），自触发器向下展开约 320 px 的 `ActivityPopover`，用户可再收为单一角标。Popover 不得覆盖 Composer、ContextMeter 或审批主操作；底部 StatusBar 触发是历史实现记录，不作为视觉验收依据（见 §8.5 与 UI_Review F-12）。
- 首行显示 Changes 的文件数与 `+added / −removed`；下方列出当前 Task 关联的 Main / subagent 状态，状态至少覆盖 running、waiting approval、completed、failed、cancelled。
- 点击 Changes 摘要重新展开 Inspector 并定位 Changes；点击 Agent 行切换到对应 Task / Agent 详情。浮窗只承载摘要，不复制 diff、Terminal 或完整 Timeline。
- Changes 摘要依赖 S8 diff projection，Agent 列表依赖 S11 多 Agent projection；能力未接通时隐藏对应分区，不能使用截图中的演示数值。

## 6. 三栏职责

| 区域 | 职责 | 阶段边界 |
| --- | --- | --- |
| TaskRail | 范围、紧凑双分组菜单、日期内项目分组、全局 / 项目定向新建、任务选择、连接状态 | S7 壳层；双分组依赖真实 Session/Workspace 投影 |
| WorkspaceView | 会话标题、Run 状态、对话、工具活动、内嵌审批、Composer、ContextMeter | S7 主路径；未知上下文不得伪造 |
| RunStatusBar | Task tokens、quota、tokens/s、Run duration | 有权威字段才显示；quota 完整面随 S11；不重复模型选择 |
| InspectorToolTabs / ActivityPopover | Changes、Terminal、Agent activity 与后续 Inspector surface | Changes S8；Terminal S10；Agent 列表 S11；能力驱动启用 |
| Composer 扩展 | 附件、`@file`、工作目录上下文 | 分别按 S9/S10 任务书接入，不因截图提前实现 |

## 7. 实现验收

- 两种组织方式消费同一份 Session projection，并有日期 → 项目 → Task 排序、空态、未归类与菜单切换定向测试。
- GroupingMenuButton、AddTaskButton 与 ProjectAddTaskButton 有 tooltip、accessible name、键盘路径及禁用原因；项目定向创建绑定正确 workspace_id。
- 切换方式后 active session 与 Composer 草稿保持不变。
- ContextMeter 使用当前上下文估算而非 Session 累计 token；未知容量不显示伪进度。
- RunStatusBar 不重复 Composer 的模型 / reasoning，并对 quota `Unknown`、无 tokens/s、Run 时间戳不完整与窄窗口溢出提供可观察回归。
- Inspector 顶层与 Changes 二级 tab 层次不可混用；折叠态 ActivityPopover 的摘要跳转、Agent 状态与 capability 缺失均有定向测试。
- 在 `1440 × 1024` 对照 v3 截图做视觉验收（三状态像素级 99% 门禁）；`1080 × 720` 为响应式**功能**门禁：验证 Inspector / ActivityPopover 切换、日期内项目分组、状态栏收敛与紧凑 Composer 可用，主操作与焦点可达，无裁切、遮挡或状态栏溢出，Connected 与断线边界态均须取证；1080 不与 1440 定稿图做像素对比（UI_Review D-03）。
- 交互态与浮层菜单按 §8 验收:hover / active 色值来自 theme token、菜单单开互斥、Escape 与外点关闭、浮层滚轮无穿透、回底控件脱钩可见 / 回底隐藏。
- 后续若确需改变设计，必须先取得用户明确批准，再更新本目录与 [GUI 设计](../docs/gui-design.md)；不得先改基准追认实现漂移。

## 8. 交互态与浮层菜单(2026-08-24 增补,旧 V3 R8 波 B)

本节记录 2026-08-24 的交互基准增补；当前像素对照与全状态复验统一由新 [R7–R8](../plan/R7-R8-ui-quality-gates.md) 执行。

### 8.1 hover / active 交互态

- 范围:全部可点控件——按钮(Send / Cancel / Reconnect / 审批三钮 / 新建角标 / Inspector 开合 / 终端 Start 等)、菜单触发器、菜单选项行、Task / 项目列表行。禁用态不加 hover。
- 色值全部来自 theme token,不引入散置字面量:

| 控件底色 | hover 背景 | token |
| --- | --- | --- |
| 无底色(ghost / 角标 / 「···」) | `#10171c` | `surface.raised` |
| `surface.raised` 控件(`#10171c`) | `#182229` | `surface.hover`(新增) |
| `accent.primary` 主按钮(`#2f6fed`) | `#3270e8` | `accent.hover`(新增) |
| `semantic.success_bg`(`#3d7a4a`) | `#438251` | `semantic.success_hover`(新增) |
| `semantic.danger_bg`(`#8a3b32`) | `#9c463c` | `semantic.danger_hover`(新增) |
| 菜单选项行(未选中,`bg.menu` 上) | `#10171c` | `surface.raised` |
| 菜单选项行(选中) | 保持 `accent.primary`,不再叠加 hover | — |

- active(按下)态复用同行 hover 色,不新增 token;hover / active 只改背景,不改尺寸、描边与文字色,不引起布局移动。

### 8.2 浮层菜单形态

- 五组菜单(grouping / scope / model / 条目「···」/ workspace 确认)统一为 gpui `deferred(anchored())` 浮层,不再占布局流,开合不改变下层内容位置。
- 面板样式沿用现状:`bg.menu` 底、`border.strong` 描边、`rounded_md`;触发器下方对齐,近窗口边缘时按 anchored 自带规则翻转 / 吸附。
- 同一时刻至多一个菜单打开:开新即关旧(修复既有「model 与 grouping 可双开」互斥不对称)。
- 关闭路径:选择选项、再点触发器、`Escape`、点击浮层外区域;§3.6 的菜单键位承诺由此落地 Escape,菜单内 ↑/↓ 导航维持缺口至波 E 键盘走查。
- 遮挡:菜单打开时拦截下层点击与滚轮(滚轮无穿透到下层滚动容器);菜单项超出时菜单自身滚动。
- workspace 确认菜单维持「无独立触发器、新建任务缺工作区时条件打开」语义,仅形态迁为浮层。

### 8.3 回底控件(FollowScroll)

- Timeline 与终端输出区:用户向上滚动即脱钩自动跟随;脱钩后该区域右下浮出「↓ 回到底部」控件(`surface.raised` 底、`text.primary` 字、`rounded_md`,带 hover 态)。
- 点击回底控件:滚动到底并重新挂接自动跟随;用户自行滚回底部同样重挂;跟随状态下控件隐藏。

### 8.4 Timeline 虚拟化与长文本截断(2026-08-24 增补,旧 V3 R8 波 C)

- Timeline 条目改由 gpui `list()`(变高行、`ListAlignment::Bottom`)承载,替换全量 eager 渲染;可视区外条目不物化,滚动性能不再随会话长度退化。
- 视觉与交互不变:条目样式、间距、审批卡位置(滚动内容末尾)与 §8.3 跟随语义保持——贴底时新内容自动跟随,用户上滚脱钩并浮出回底控件,回底重挂;长会话下唯一可感知差异是滚动流畅性。
- 条目内交互(「···」菜单、审批按钮)与焦点行为不回归:菜单仍为 §8.2 浮层,审批按钮保留 tab stop 与 tooltip。
- 长标题截断:TaskRail 的 Task 标题与项目头名称单行省略号截断(不换行、不撑高行),截断只发生在侧栏宽度不足时;主区 Timeline / Composer 不渲染标题,不受影响。

### 8.5 Changes / Resources 面板与 `@` 引用(2026-08-24 增补,旧 V3 R8 波 D)

- **Inspector 顶层 tab strip**:Changes / Terminal / Resources 三个固定文本页签(§5 的 Add tool 动态注册管理本波不实现,Resources 先以固定页签呈现);R6 Wave A 起为 100×58 文本页签、选中态底部 2px accent 下划线、字阶 `font::BODY` 18，不再使用 raised/ghost 按钮形态;切页签不改 active session,各页签独立保留滚动与展开状态;cmd-i 开合 Inspector 的既有行为不变。固定 Resources 页签是过渡实现记录，不等于定稿图「+」(Add tool) 入口已达成：Add tool 只在 Host 提供 Inspector surface 注册 capability 后以 capability-driven 形态出现，此前 F-10 保持未通过，不得以 Resources 固定页签顶替验收（UI_Review D-02）。
- **Changes 二级页签**:Files / Summary 为 Changes 内容区内的二级文本页签，固定 96×56、字阶 `font::BODY_SM` 17、选中态同样 2px accent 下划线，与顶层 58px 层次不混用(§5 既有红线)。
- **Files 页**:逐文件一行(路径单行 truncate、status、`+added / −removed`),点击行选中后经 diff_get 拉取该文件 hunks;全部数据来自 Host 响应,无会话或无 diff 时空态文案,不画演示数。
- **DiffView**:hunk 头(`@@` 行)surface.raised 底 + text.secondary;行级语义着色——新增行 semantic.success 系、删除行 semantic.danger 系、上下文行 text.primary;等宽字体为 DiffView 显式指定(`font::MONO` = Menlo;Terminal 页输出仍走 GPUI 默认字体,二者并非同款);长行不换行,容器横向滚动(全仓首个 `overflow_x_scroll` 用例,横滚 extent 行为列入新 R6/R8 验证矩阵);binary / 不支持状态按响应字段如实标注,不尝试渲染。
- **Summary 页**:会话 diff 聚合(文件数、总 `+A / −D`、按 status 分组计数)与响应携带的 git 信息(branch、dirty 文件数);字段缺失显示 unknown,不伪造。
- **ActivityPopover**:Inspector 折叠时由 Workspace Header 右上 Activity 触发器向下弹出(§8.2 浮层形态:deferred(anchored())、Escape/外点关闭、occlude 滚轮无穿透),宽约 320px 且不覆盖 Composer;首行 Changes 摘要(N files · +A/−D),点击展开 Inspector 并定位 Changes 页;Agent 状态仅在 Host capability 存在时显示;摘要未拉取或来源不可用时显示 unavailable,不显示 0。StatusBar 底部触发是历史实现，不是定稿目标。
- **Resources 页**:MCP server 只读列表(name、transport、state、tools 数、last_error 诚实显示);空列表空态文案;「已加载规则」分区无 Host 出口,本波不画。
- **`@` 引用**:composer 输入 `@token` 不弹候选浮层(补全留候选);发送后由 Host 展开为独立 Text part(`[attached file: path (marker)]` + 正文),Timeline 用户消息按 parts 顺序拼接渲染(与 CLI 历史语义一致),附件正文随消息展示、不另起条目、不做折叠。

### 8.6 主题残余 token 补齐(2026-08-25 增补,旧 V3 R8 整阶段审计)

- **text.placeholder**：当前生产实现仍为白色 30% 透明（`Rgba { r:1, g:1, b:1, a:0.3 }`，与既有 `hsla(0,0,1,0.3)` 字面量逐值相等）；这是迁移审计的历史现状，不再是 R2 目标。R2 按 §2.1 改为不透明 `#7f7f7f` 并跑真实组合色对比度门禁。
- **metrics::SUMMARY_LABEL_WIDTH**:Changes · Summary 行标签列宽 `88px`,changes.rs 字面量回收入 metrics;同批 `metrics::ZERO` 用于 dropdown 锚点偏移与行高保护性比较。均为行为不变的字面量归位,视觉零变化。
