# R4–R6 — 核心 Agent 工作流

> 状态：⚪ 未开始
> 前置：R2、R3 依次通过。R4 → R5 → R6 串行推进，每阶段同时完成视觉、真实交互与对应 UI 场景测试。

## R4 — Workspace、Timeline 与 Agent 状态

### 工作范围

- Workspace Header：真实 task 标题、branch/workspace、运行终态与右侧动作；缺字段只隐藏该项，不删除整体骨架。
- Timeline：user/assistant 的段落、列表、代码、长行、时间/身份层级与 readable width；短会话从 Header 下开始，流式 follow 与用户回看互不抢滚动。
- Tool activity：按真实事件组合 pending/running/succeeded/failed/cancelled，参数/结果可展开；不将日志字符串伪装为结构化状态。
- Approval：在 transcript 内显示动作、目标、越界原因、风险与允许/拒绝选择；进入等待时 TaskRail 与 Activity 同步为 Needs input，决策可回放。
- Error/cancel/completion：明确失败原因、retry 条件、取消终态与 Ready for review 摘要；不用不可验证百分比。
- 大规模 Timeline：千级事件虚拟化、选择、展开、追加、断线重放与 window close 后恢复。

### 关键场景

短会话、长会话、流式追加、用户上滚后新消息、tool 全状态、approval allow/deny/cancel、Host error/retry、断线重连、完成摘要与多 task 状态同步。

### R4 退出标准

- [ ] Header、消息、tool group、approval、错误/取消和完成摘要均与设计层级一致且由权威事件驱动。
- [ ] State A/B Timeline 分区结构通过，区域 SSIM `≥0.99`。
- [ ] 每种 Agent 状态都有 U0/U1/U2 场景；重放后 UI 与实时执行一致。
- [ ] 长会话无巨幅空白、无限行宽、截断、滚动抢夺或明显输入延迟。

## R5 — Composer 与运行控制

### 工作范围

- 常态总高、增长上限、输入区、footer controls、ContextMeter 与 Send/Cancel 几何严格按定稿图；`88–94px` 是常态总高，不是输入框最小高。
- 完整输入：mouse/keyboard focus、多行、选择、复制粘贴、撤销重做、中文/日文 IME composition、超长文本与拖放/附件（仅 capability 已实现时）。
- 真实控制：model/provider、reasoning、workspace/context chip、`@` 引用、发送、取消与 disabled/loading/error；选择项从 registry/capability 派生。
- 运行中 follow-up/queue 只有 Host 支持才显示；否则保持可解释禁用，不模仿 Codex 的云端/后台能力。
- 快捷键可发现；Enter/Shift-Enter、Esc、审批键与菜单导航不能互相冲突。
- 状态切换不得移动 Composer 锚点、拉伸按钮、吞掉草稿或让 Popover 覆盖主操作。

### 关键场景

空输入、普通发送、多行/IME、粘贴大文本、切换模型/reasoning/workspace、`@` 选择与移除、发送中取消、错误重试、断线草稿保留、task 切换草稿策略、窄窗和键盘全路径。

### R5 退出标准

- [ ] Composer 所有可见控件真实可用或诚实不可用，生产路径无演示数据与空点击面。
- [ ] State A/B 常态与 running Composer 几何/视觉在容差内，区域 SSIM `≥0.99`。
- [ ] IME、paste、多行、草稿、send/cancel、菜单和 focus 恢复场景稳定通过。
- [ ] 任何状态均无按钮拉伸、遮挡、布局跳动或输入丢失。

## R6 — Inspector、Changes、Terminal 与 Activity

### 工作范围

1. **Inspector 壳**：顶层 tab 与 Files/Summary 二级 tab 层级清楚；展开/折叠后选择、scroll、terminal session 和 focus 可恢复。
2. **Changes**：明确 working tree / last turn 等真实 scope；文件摘要、选中行、DiffView、行语义色、长行横滚和底部动作。stage/unstage/hunk 若协议未定义，不出现假可用按钮。
3. **Terminal**：绑定当前 workspace/session，覆盖创建、输入、输出、resize、停止、失败与重连；Policy/approval 语义真实，不能绕过 Host。
4. **Resources / Add tool**：仅展示 Host 已提供的 MCP/resource/capability；与设计稿 `+` 的关系按 R1 决议，缺少候选查询时不伪造完整选择器。
5. **Activity**：触发器固定 Workspace 右上；折叠 Inspector 后向下打开约 320px Popover，显示权威摘要与恢复入口，不覆盖 Composer。
6. **状态一致**：TaskRail、Header、Timeline、Inspector/Popover 对 Running/Needs input/Ready/Blocked/Disconnected 使用同一语义。

### 关键场景

Changes 空态与真实 diff、文件切换、Summary/DiffView、长行横滚；Terminal 全生命周期；Resources 空/可用/失败；Inspector 折叠/恢复；Activity 打开/关闭/窗口边界；断线重连与 task 切换。

### R6 退出标准

- [ ] State A Inspector 与 State B ActivityPopover 结构、锚点、层级和内容密度对齐；各区域 SSIM `≥0.99`。
- [ ] Changes/Terminal/Resources 的展示范围与真实 capability 一致，无越权、假动作或错误 scope。
- [ ] 所有 tab、二级 tab、菜单、滚动、折叠、恢复和键盘路径有 U1/U2 场景。
- [ ] Terminal/approval/断线不会破坏 Run 生命周期、协议边界或安全决策。
