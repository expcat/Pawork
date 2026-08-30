# R7–R8 — 全局交互质量与 UI 终局比对

> 状态：R7 🟢 已关闭（主动系统偏好 U3 依用户指令 ⏭️；VoiceOver 未执行且不宣称通过）· R8 ⚪ 待开启
> 前置：R1–R6 已依次退出，R7 已关闭。2026-08-31 用户重排 R8–R11：原 R11（UI 终局比对与优化文档）前提到 R8 位置，先与设计稿对比；原 R8（模拟操作全功能验收）连同全部「移交 R8」条款（R2–R6 分区 SSIM、fixture 演示数据重塑、State C reference 底色拍板、用户视觉签字）并入 R10 测试，见 [R9–R11 任务书](R9-R11-post-ui-closeout.md)。移交项不得记为已经通过。

## R7 — 全局交互、Accessibility 与响应式

### 执行波次

- **Wave A（🟢 2026-08-29–30）— 组件状态矩阵与 AX 基线**：自动门禁已通过（45 组件矩阵、三路径焦点修复、U2 26 相位、A3 bundled/签名对照、A4 State A hover/active/focus 九图）；人工 overlay 续查发现并修复 Inspector 顶层页签无可见 hover 的缺口，2026-08-30 用户确认九图通过。用户同时批准本波以原生 AX tree/action + 纯键盘 + U2 替代 VoiceOver；VoiceOver 未执行、不记为通过，屏幕朗读措辞 / 顺序仍未验证。写入限 apps/desktop、Desktop Spec、R7 测试脚本与本波证据；未改 GUI wire、Host、Policy 或 fixture 业务数据。macOS 26 AX 递归劣化已 fail-closed 取证（attempt7–10），不以重启成功冒充根治。
- **Wave B（🟢 2026-08-30）— 全局 focus、菜单与 Popover 等价路径**：基于 Wave A 45 组件矩阵、R3 导航 U2 与 R6 Inspector U2 核对现状，收敛六个真实焦点缺口：task 切换关闭旧菜单并聚焦 Composer，审批 action 关闭菜单并聚焦 Composer，Review changes 展开 Inspector 后聚焦 Changes 选中页签，Fork 接受后聚焦 Composer；独立审查再补当前 task 的 AXPress 关闭菜单，以及仅一个可见 task 时 cycling 不重开 session 但仍聚焦 Composer。mouse / keyboard / AX 继续汇入既有 handler 与 enable gate。Desktop 144/144、Python 17/17 + 22/22、导航 26 相位、审批/状态 14 相位及审查边角 3 相位真窗口 U2 全绿；首次审批长驱动暴露的是 R4 对“空输入 Send enabled”的过期断言，按 R5 冻结合同改为 disabled 后同驱动全绿。审查边角仅在隔离临时 fixture root 通过既有 `archived` 字段构造单可见 task，仓库 seed 与 fixture 业务数据未改。证据见 [r7-wave-b](../docs/ui-review/r7-wave-b/notes.md)。未改 GUI wire、Host 或 Policy。
- **Wave C（🟢 2026-08-30）— 响应式、长内容与平台偏好**：默认平台态真窗口子集覆盖 1080×720 Connected / ActivityPopover / Disconnected、CJK/emoji/超长内容、1024 行虚拟化与离底/回底、三轮宽窄 resize、焦点保持、重连及单次性能基线；连接长文案最终以定宽槽 + 截图级 paint 门禁收口。字体 token 从固定 px 改为 rem，新增 `Cmd+=` / `Cmd++`、`Cmd+-`、`Cmd+0` 的应用内 100%/125%/150% 缩放；消息行高按 24px 基准随字号缩放，150% 最小窗使用 320px rail，Workspace 保留 760px，Task 标题/日期间距与行高修复后受影响区域真窗口 U2 均通过。macOS Increase Contrast 读取与显示选项变更刷新已落地，默认 token 不变；当前 UI 无动画，Reduce Motion 无需渲染分支。用户最终要求跳过一切需要修改系统设置的测试并恢复原值，因此主动 Reduce Motion / Increase Contrast U3 记为 ⏭️ 而非通过，最终只读快照四项均为 `false`。正式 seed 未改，千级数据只派生于隔离临时数据库。证据见 [r7-wave-c](../docs/ui-review/r7-wave-c/notes.md)。

### 1. 交互状态

- 对 manifest 中每个 button、row、tab、menu item、popover、input、splitter、scroll area 定义 default/hover/active/focus/disabled/loading/error/selected。
- 点击面、cursor、tooltip、press feedback、menu 锚点和 dismissal 一致；浮层不能被父级裁切，不能改变底层布局。
- focus 顺序按 TaskRail → Workspace → Composer → Inspector；面板开合、菜单关闭、task 切换、审批结束后恢复到可预测位置。
- command palette、面板 toggle、task cycling、next-needs-attention 与审批快捷键可发现、可重映射或至少集中登记。

### 2. Accessibility

- 为所有交互元素提供稳定 identifier、AX role/name/value/state；图标按钮必须有名称，状态不能只靠颜色。
- 纯键盘可完成连接重试、新建/选择 task、发送/取消、tool 展开、审批、Changes、Terminal、Inspector/Activity 和菜单关闭。
- 原生 AX audit + VoiceOver 验证 role/name/value/enabled/focused/order/action，以及动态状态、流式消息、审批请求、错误和完成通知；避免重复播报整条 Timeline。AX 树只含 Window/traffic lights 时直接失败。R7 Wave A 依用户 2026-08-30 决定，以原生 AX tree/action + 纯键盘 + U2 作为该波替代门禁；这不等于 VoiceOver 通过，也不覆盖屏幕朗读措辞 / 顺序，R10 的系统级口径另行执行。
- 文字/状态/焦点对比度、字号放大、reduced motion 与高对比偏好按平台可用能力验证；R7 已实现应用内 100%/125%/150% 缩放和 macOS Increase Contrast 运行时 palette 刷新，当前 UI 无动画故无 Reduce Motion 分支。依用户指令，主动系统偏好 U3 未执行且不得当作已证明；官方竞品未公开的 AX 行为同样不得当作已证明。

### 3. 响应式与耐久性

- `1440×1024` 是 99% 视觉主门禁；`1080×720` 验证折叠顺序、主操作可见、Popover/菜单不越界。
- 超长标题、CJK/emoji、长代码行、空/错误/断连态、千级列表、连续流式和反复 resize 不截断关键动作、不泄漏焦点。
- 记录启动到可交互、长列表滚动、输入响应、resize 与 screenshot 稳定时间；阈值在 R1 基线后冻结，禁止用固定长 sleep 掩盖抖动。

Wave C 已归档 `baseline_only` 真实机器采样；该样本只建立量测入口，阈值保持 `null`，不能据此宣称性能无回退。字号放大真窗口路径已覆盖；主动平台偏好态依用户指令跳过，不计为通过。

### R7 退出标准

- [x] 组件状态矩阵没有缺口；mouse、纯键盘和 AX 三条主路径等价。
- [x] State A/B/C 的 hover/active/focus/menu/popover 补充图通过人工 overlay。
- [x] `1080×720`、字号放大、长内容和边界状态无主操作遮挡、溢出或不可恢复焦点。
- [x] AX tree、通知和 focus trace 可自动留证；VoiceOver 与主动系统偏好 U3 的未执行边界已明确登记。

## R8 — UI 终局比对与优化文档

R8 是文档任务。对照 [design/](../design/README.md) 三张 v3 定稿图与 R1–R7 各波已归档的实际 UI 证据，从**结构、UI 组件样式、真实美观度**三个维度评估实际 UI 与设计效果的差距，并参考主流 Agent/开发者工具与设计体系的公开样式实践，输出一份 UI 优化文档（`docs/ui-optimization.md`），告诉后续 Agent 该优化哪些 UI、样式与风格。本阶段**不查询、不修改任何代码**；不启动 Desktop、不重跑 cargo、不重拍 current、不改 design。发布准备不属于本阶段，见 [ROADMAP §5](../ROADMAP.md)。

R8 不替代 [R10 退出标准](R9-R11-post-ui-closeout.md#r10-退出标准) 的 99% 门禁与全功能 suite；也不在本阶段修复差异。UI 优化文档是 R9 修复阶段的输入，本身不授权实现。

### 比对输入（只读）

- `design/`：Timeline（Inspector 展开）、Timeline（Inspector 折叠）、Projects 三张 v3 定稿图。
- [docs/gui-design.md](../docs/gui-design.md) 的信息架构与交互规则（用于判断缺状态、缺分区，而不只看像素）。
- [docs/UI_Review.md](../docs/UI_Review.md) 的分区、容差与结构一票否决（用于区分合同内误差与需完善项）。
- R1–R7 各波归档的三状态 `reference` / `current` / overlay / diff / mask / checklist（以最新一波为准）；已知 fixture 演示内容形状差按既有记录登记为数据缺口，移交 R10 前置处理。不得打开 `apps/desktop` 或其它 crate 源码定位组件。
- [Agent UI 参照调研](UI-reference-research.md) 的交互经验，以及 Codex、Zed、Cursor、VS Code 等主流产品和主流设计体系/组件库的**公开**视觉样式资料（官方文档/截图/HIG 等）；营销图只作线索，不作事实。

### Wave A：逐区对照（结构 + 组件样式）

- 按 State A/B/C 与 UI Review 分区（header、TaskRail、timeline、composer、inspector、statusbar 等）对照 design 与 current。
- 只登记**显示效果**：布局、色/字/间距、圆角/描边、图标、文案可见性、组件有无、状态外观。截图上看不出的交互缺口标「截图无法判定」，不查代码补证。
- 容差内、已遮罩、或前置波次已明确接受的项不重复立项；结构未对齐或仍刺眼的可见差异必须登记。
- 每条写：区域、design 期望、当前现象、证据路径（design 资产 + current/diff）、建议优先级。禁止指向源码路径或「应改某函数」。

### Wave B：真实美观度与主流样式对照

- 在真实 UI 证据上评估美观度：视觉层级是否清楚、密度与节奏是否舒适、对比/留白/对齐/分隔是否精致、深色 surface 层级是否分明、控件质感是否统一。
- 对主流产品与设计体系做公开资料级样式扫描（组件质感、字阶、状态色、浮层、空态、密度档位等），与 Pawork 同类组件并排对照，提炼可吸收的样式经验；不复制品牌、文案、竞品专属能力或 Pawork 未接入能力的入口。
- 每条写：主题、主流做法（附公开来源）、Pawork 现状与证据路径、吸收建议与优先级；与 Wave A 差异去重合并。

### Wave C：输出 UI 优化文档

- 汇总 Wave A/B 为 `docs/ui-optimization.md`：分区差异清单 + 主流样式对照 + R9 修复任务草案（一任务一缺口族，数小时内可完成，含证据链接、优先级与验收线索）。
- 优化文档面向后续 Agent：明确该优化哪些 UI、样式、风格，以及不属于优化范围的内容（design 基准变更、wire/能力扩张、品牌与文案照搬等）。
- 回写 [ROADMAP.md](../ROADMAP.md) 下一指针（R9 修复）。
- 本阶段 git 差异仅文档。不得把 License、安装器、供应链或全量门禁塞进本阶段。

### R8 退出标准

- [ ] 三张定稿图与对应 current 证据已逐区对照，结构与组件样式差异形成清单。
- [ ] 真实美观度评估与主流样式对照完成，可吸收经验已提炼并标注公开来源。
- [ ] UI 优化文档（`docs/ui-optimization.md`）已产出：告诉后续 Agent 该优化哪些 UI、样式、风格，含证据链接与优先级。
- [ ] 本阶段未查询、未修改代码；design 像素未改。
