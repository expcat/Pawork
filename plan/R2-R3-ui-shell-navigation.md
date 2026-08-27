# R2–R3 — Window shell、全局视觉系统与 TaskRail

> 状态：🔵 R2 进行中（Wave A 🟢 2026-08-27：F-01 透明 titlebar、F-02 三栏骨架、§2.1 根 token 落地、1440/1080 layout invariant、State A shell 证据，见 [../docs/ui-review/r2-wave-a/notes.md](../docs/ui-review/r2-wave-a/notes.md)）；R3 ⚪ 未开始
> 前置：R1 全部退出标准通过。两个阶段严格串行；R2 完成后才开启 R3。

## R2 — Window shell 与全局视觉系统

### 目标

先把整窗骨架对齐定稿图，再让后续组件在同一视觉语法上生长。Pawork design 决定外观；Codex 等参照只影响面板操作和状态反馈。

### 工作范围

1. **Window chrome**：macOS traffic lights 与深色壳融合；内容视口不混入额外白色 titlebar 高度；焦点/失焦态不闪白。
2. **三栏几何**：TaskRail `288px` 基准、弹性 Workspace、Inspector 约 `440px`；分隔线、顶部 Header 区与底部 StatusBar 连续对齐。
3. **视觉 token**：逐项校准 surface、文字层级、边框、状态色、字体/行高、图标、圆角、阴影和重复间距；禁止局部硬编码造出第四套 token。
4. **全局布局**：Header、Timeline、Composer、Inspector 与 StatusBar 各自拥有稳定边界；折叠 Inspector 不改变 Composer 锚点或产生双线。
5. **窗口状态**：启动、连接中、无 Host、空 task 与窗口失焦均继承同一壳层，不以调试面板抢占主视觉。
6. **响应式底线**：在 `1440×1024` 精确对齐主图；`1080×720` 不遮挡主操作、不溢出、不丢失焦点指示，具体折叠顺序按 R1 合同执行。

### 模拟操作与证据

- U1：窗口组件树、三栏宽度、边界、focus ring、折叠动作和 `1440/1080` layout invariant。
- U2：启动/关闭/重开、focus/blur、resize、Inspector 开合、连接失败重试；不得依赖固定 sleep。
- U3：空态 + State A/B 的 shell 分区截图、overlay/diff、AX tree；Window chrome 结构错误直接失败。

### R2 退出标准

- [ ] `1440×1024` 壳层结构 100% 对齐，所有区域几何在 UI Review 容差内。
- [ ] 整窗无白带、重复 titlebar、布局跳动、面板溢出或主操作遮挡。
- [ ] State A/B 的 shell 各区域 SSIM `≥0.99`，且结构/overlay 人工复核通过。
- [ ] 启动、连接失败、失焦、resize 与 Inspector 开合均有可重复模拟操作测试。

## R3 — TaskRail 与任务导航

### 目标

按定稿图完整实现 Timeline/Projects 两种组织方式，让用户在打开 transcript 前即可识别 Running、Needs input、Ready、Blocked 与 Unread；状态必须来自 Host，不用颜色或假文案推断。

### 工作范围

1. **顶部层级**：Pawork 标识、Grouping 触发器、scope、连接状态与全局新建 task；点击面、字阶、图标槽和三行节奏与图一致。
2. **Timeline 模式**：日期 → project → task；长标题、省略、状态图标、选中、hover、unread/attention 与滚动恢复完整。
3. **Projects 模式**：project header、task count、定向 `+`、展开/收起、选中、空 project 与长列表；不得用 raw path 或重复 `New session` 代替真实标题。
4. **底部账户区**：只显示已有权威账户/本机身份与设置入口；不存在的 quota/组织/远程状态隐藏或标记 unavailable。
5. **导航状态**：新建、打开、切换 scope/grouping、连接重试、断线保留 selection、重连恢复、删除/归档后的焦点回退。
6. **键盘**：Tab/Shift-Tab、方向键、Enter/Space、Esc、task cycling 与 next-needs-attention；快捷键可发现且不复制某一平台的不可改硬编码。

### 模拟操作场景

- Timeline → Projects → Timeline，断言选中 task、滚动位置和 focus 恢复。
- project 定向新建与全局新建，断言 workspace 归属、标题和列表插入位置。
- Running → Needs input → Ready 状态跨 TaskRail/Timeline/Activity 一致更新。
- 断连、重连、空列表、超长标题、百/千级 task、窄窗与菜单越界。
- 鼠标、纯键盘和 AX 语义三条路径完成相同主任务。

### R3 退出标准

- [ ] State A/B/C 的 TaskRail 结构、密度、选中和状态 100% 对齐定稿图。
- [ ] Grouping/scope/新建/project/task/账户区的所有可达控件均有真实动作或诚实不可用态。
- [ ] Timeline/Projects 的 selection、scroll、focus、重连恢复和长列表没有回退。
- [ ] 三状态 TaskRail 分区 SSIM `≥0.99`，组件状态矩阵与模拟操作测试全绿。
