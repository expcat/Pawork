# R7–R8 — 全局交互质量与模拟操作全功能验收

> 状态：🔵 R7 Wave A 自动门禁已通过、暂停等待 R6 退出后恢复人工验收 · R8 ⚪ 未开始
> 前置：默认要求 R1–R6 逐阶段通过；用户于 2026-08-29 明确授权跳过当时未收口的 R6、直接进入 R7。R6 Wave B 的实现、定向门禁与审查后最终 U2 已于 2026-08-30 补齐，但 R6 阶段仍等待 State A/B 视觉门禁移交确认；此前跳过授权不构成该视觉决定。R7 解决跨组件一致性，R8 运行完整矩阵并补跨阶段缺口，均不得把前置未通过项直接记为通过。

## R7 — 全局交互、Accessibility 与响应式

### 执行波次

- **Wave A（🔵 2026-08-29，暂停）— 组件状态矩阵与 AX 基线**：自动门禁已通过（45 组件矩阵、三路径焦点修复、U2 26 相位、A3 bundled/签名对照、A4 State A hover/active/focus 九图）；VoiceOver/overlay 仍待人工签字，待 R6 退出后恢复。写入限 apps/desktop、Desktop Spec、R7 测试脚本与本波证据；不改 GUI wire、Host、Policy 或 fixture 业务数据。macOS 26 AX 递归劣化已 fail-closed 取证（attempt7–10），不以重启成功冒充根治。
- **Wave B（⚪）— 全局 focus、菜单与 Popover 等价路径**：收敛 hover/active/focus/disabled/loading/error/selected、单开浮层、Escape/外点 dismissal、可发现快捷键与面板/菜单/审批后的焦点恢复；同一 action 必须复用既有 enable gate。
- **Wave C（⚪）— 响应式、长内容与平台偏好**：覆盖 1080×720、字号放大、CJK/emoji/超长行、千级列表、反复 resize、reduced motion/高对比偏好与性能基线；只记录平台真实能力，不伪造跨平台 AX 支持。

### 1. 交互状态

- 对 manifest 中每个 button、row、tab、menu item、popover、input、splitter、scroll area 定义 default/hover/active/focus/disabled/loading/error/selected。
- 点击面、cursor、tooltip、press feedback、menu 锚点和 dismissal 一致；浮层不能被父级裁切，不能改变底层布局。
- focus 顺序按 TaskRail → Workspace → Composer → Inspector；面板开合、菜单关闭、task 切换、审批结束后恢复到可预测位置。
- command palette、面板 toggle、task cycling、next-needs-attention 与审批快捷键可发现、可重映射或至少集中登记。

### 2. Accessibility

- 为所有交互元素提供稳定 identifier、AX role/name/value/state；图标按钮必须有名称，状态不能只靠颜色。
- 纯键盘可完成连接重试、新建/选择 task、发送/取消、tool 展开、审批、Changes、Terminal、Inspector/Activity 和菜单关闭。
- 原生 AX audit + VoiceOver 验证 role/name/value/enabled/focused/order/action，以及动态状态、流式消息、审批请求、错误和完成通知；避免重复播报整条 Timeline。AX 树只含 Window/traffic lights 时直接失败。
- 文字/状态/焦点对比度、字号放大、reduced motion 与高对比偏好按平台可用能力验证；官方竞品未公开的 AX 行为不得当作已证明。

### 3. 响应式与耐久性

- `1440×1024` 是 99% 视觉主门禁；`1080×720` 验证折叠顺序、主操作可见、Popover/菜单不越界。
- 超长标题、CJK/emoji、长代码行、空/错误/断连态、千级列表、连续流式和反复 resize 不截断关键动作、不泄漏焦点。
- 记录启动到可交互、长列表滚动、输入响应、resize 与 screenshot 稳定时间；阈值在 R1 基线后冻结，禁止用固定长 sleep 掩盖抖动。

### R7 退出标准

- [ ] 组件状态矩阵没有缺口；mouse、纯键盘和 AX 三条主路径等价。
- [ ] State A/B/C 的 hover/active/focus/menu/popover 补充图通过人工 overlay。
- [ ] `1080×720`、字号放大、长内容和边界状态无主操作遮挡、溢出或不可恢复焦点。
- [ ] AX tree、通知和 focus trace 可自动留证；已知平台限制明确登记。

## R8 — 模拟操作全功能验收

### 1. 全量场景矩阵

| 领域 | 必须模拟的操作与状态 |
| --- | --- |
| 启动与连接 | 无 Host、连接、失败重试、断连、重连、window close/reopen；区分 persisted/connected/executing/blocked |
| TaskRail | Timeline/Projects、scope、project 展开、全局/定向新建、task 切换、selection/scroll/focus 恢复、Unread/Needs input |
| Composer | click/type、多行、IME、paste、model/reasoning/workspace/context/`@`、send、cancel、草稿与不可用态 |
| Timeline | stream、tool 全状态、展开/收起、approval allow/deny、error/retry、cancel、completion、follow-scroll 与千级事件 |
| Changes | 空态、真实多文件 diff、Files/Summary/DiffView、长行横滚、scope 与只读动作 |
| Terminal | create、input/output、resize、stop、失败、task/workspace 切换、重连与 Policy 拒绝 |
| Resources | 空/可用/失败、resource 打开、Add tool/capability 缺失的诚实状态 |
| Inspector/Activity | tab、二级 tab、折叠/恢复、右上 Popover、dismiss、焦点/滚动/session 保持 |
| 浮层与快捷键 | grouping/scope/model/reasoning/`@` 菜单，command palette，Tab/方向键/Enter/Esc，窗口边界与 outside click |
| 响应式/AX | State A/B/C 的 1440 图、1080 窄窗、字号放大、纯键盘、VoiceOver/AX、状态非纯颜色 |
| 生命周期 | Run 中关闭窗口、Host 仍运行、重开恢复、approval 等待恢复、完成通知与后台状态真实性 |

每一行至少覆盖成功、失败/拒绝和恢复路径；所有 manifest 组件及可达状态必须能反查到场景 ID。单条 happy path、只测 renderer 或人工随意点击均不构成“全功能”。

### 2. 执行策略

- 使用 R1 固定 seed 与隔离数据；每场景独立 reset，允许按标签重跑。
- 语义定位优先：AX identifier/role/name + 明确状态等待；坐标只用于几何验证，固定 sleep 只允许有界兼容并需记录原因。
- U0/U1 先行，U2 真进程覆盖所有用户动作，U3 只对稳定终态采图；真实 Provider 不属于 UI fixture。
- PR/本地默认运行 U0/U1 与小型稳定视觉集；macOS 定时门禁运行完整 XCUITest/视觉集；R8 收口再串行执行 U0–U3、真实 IME/VoiceOver 与性能，避免并发争抢 Cargo/主线程窗口资源。
- flaky 测试不可“重跑即绿”后隐藏：记录首次失败、重试结果、随机种子和根因；同一场景连续不稳定即阻塞签字。
- failure bundle 至少含 action trace、AX tree/当前焦点、Host/event log 与协议 sequence、窗口尺寸、current/reference/overlay/diff/mask、AE/PDC/RMSE/SSIM 指标、fixture manifest、seed、OS/Xcode/GPU/scale/locale/input source/font、时间与源码状态；若使用 XCTest，同批保留失败 `xcresult`/attachments。

### 3. 视觉终局门禁

- State A/B/C 的可见区域、组件、顺序、展开/折叠和选中状态必须 100% 对齐。
- TaskRail、Header、Timeline、Composer、Inspector/Popover、StatusBar 各区域动态遮罩后 SSIM `≥0.99`；结构一票否决优先于数值。
- **R2 移交（2026-08-27 拍板 a）**：R2 只以壳层结构门禁退出，不把内容区未落地组件的分区像素差记为 R2 失败。R8 必须在 F-03–F-12 落地后重新采集 State A/B/C current，再跑分区 SSIM；不得沿用 R2 Wave A 的 0.65–0.81 中间态报告作为终局通过。
- **R3 移交（2026-08-28 拍板 c）**：R3 以 TaskRail 结构门禁退出（Wave A State A/C 结构断言全 PASS；State B 与 State A 同 Timeline 模式，未单独采 TaskRail 分区图），三状态分区 SSIM ≥0.99 不在 R3 判定。R8 重采集 current 前必须先完成 **fixture 演示数据重塑**（fixtures/ui/seed.json 数据形状对齐定稿图演示形状：标题长度/时间分布/会话数，同步 golden 与约 18 处断言引用，估算 0.5–1 天）；并就是否按冻结 token 归一 State C reference 底色另行取得用户批准（设计基准变更）。天花板量化分解：State A ≈100% 内容形状（0.6941，tone 校正上限 0.7490）；State C = tone ≈50% + 形状 ≈50%（0.3543，tone 校正后 0.6885）。遮罩侧无合规余量（已用 16.6%/14.9%，上限 35%），不得靠放宽 UI_Review §0.1 遮罩合同制造通过。细节见 [../docs/history.md](../docs/history.md#r3--taskrail-与任务导航2026-08-2728)。
- **R4 移交（2026-08-28 拍板 1）**：R4 以 Header/Timeline 结构门禁与 U2 九场景退出，State A/B 分区 SSIM ≥0.99 不在 R4 判定。R8 重采集 current 时一并覆盖 Header / Timeline / 相关 Workspace 分区；不得沿用 Wave A 记录值（timeline 0.665 / header-left 0.940 / header-right 0.883 / global 0.648）作为终局通过。主因与 R3 相同：fixture 演示内容形状差，重塑已在拍板 c 移交，本条不另开数据任务。细节见 [../docs/history.md](../docs/history.md#r4--workspacetimeline-与-agent-状态2026-08-28)。
- **R5 移交（2026-08-29 用户确认）**：R5 以 Composer 几何结构门禁、定向测试与 U2 九场景退出，State A/B Composer 分区 SSIM ≥0.99 不在 R5 判定。R8 必须用重塑后的同一 fixture 重采 current 并覆盖 idle/running Composer；不得沿用 R5 Wave A 记录值 0.423 / 0.619 作为终局通过。详见 [../docs/history.md](../docs/history.md#r5--composer-与运行控制2026-08-2829)。
- 所有 P0/P1 Review 项关闭；无白 titlebar、缺失 Header、错位 Popover、超高 Composer、假数据、遮挡、截断或布局跳动。
- 由用户在同尺寸 reference/current/overlay 上完成最终视觉签字；自动门禁通过不能代替签字。

### 4. R8 退出标准

- [ ] manifest 的组件 × 状态 × 输入方式覆盖率 100%，所有场景可独立重放并有明确断言。
- [ ] U0/U1/U2/U3 全部通过；跨进程、断连、恢复、后台 Run 与审批恢复有真实证据。
- [ ] 三张定稿图的结构门禁、分区 SSIM 与人工 overlay 全通过，无 P0/P1 遗留。
- [ ] 用户完成视觉签字；已知 P2/P3 只可在不破坏 99% 与全功能的前提下明确接受并登记。
- [ ] 失败证据、性能基线、AX 结果和实际命令归档；ROADMAP 指针移至 R9。
