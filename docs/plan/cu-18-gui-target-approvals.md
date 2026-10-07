# CU-18 · GUI 目标选择与权限交互

> 状态：已实现；自动门禁通过，真窗验收覆盖入口/空态/刷新/断线重连，empty 态与授权回执端到端待 CU-20 目标登记后人工验收（P5）。前置：[CU-03](cu-03-app-window-discovery.md)、[CU-16](cu-16-target-approvals.md)、[CU-17](cu-17-gui-observation-protocol.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

让用户选择后台操作目标并管理应用/网站授权。

## 当前依据与读取范围

当前 GUI 有工具审批，但没有本机 computer-use 目标选择和应用范围权限产品面。

[Desktop Spec](../spec/desktop.md)、[desktop crate Spec](../spec/crates/desktop.md)、[GUI 设计](../gui-design.md)、[client Spec](../spec/crates/client.md)。

## 实施范围

- 通过 client/协议展示真实应用、窗口或标签目标及支持的后台能力；选择不激活目标应用。
- 明确展示当前任务授权、持久授权与撤销状态，系统权限缺失给用户可执行指引。
- UI 不直接依赖 computer-use、数据库或 Provider；Desktop 直接 pawork-* 依赖仍符合当前红线。
- 键盘、AX、错误与旧 Host 禁用状态同源，界面不暴露不必要的内部协议细节。

## 完成条件与验证

- 用户能用鼠标/键盘选对目标，授权/拒绝/撤销与 Host 的真实决定一致。
- 选择目标或展示权限不会抢目标应用前台，也不能通过模型点击形成自动批准。
- 真实窗口检查并同步 GUI 设计与 Desktop Spec，自动测试不能替代像素验收。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-07 实施完成。

**实现**：Desktop 新增 Inspector「后台操作」页（apps/desktop/src/ui/computer.rs + controller/computer.rs），经 pawork-client 走 GUI API 1.27 的 computer_target_list / approve / revoke；面板按会话绑定拉取目标，断线保留数据并标记 stale，重连自动重查；目标卡展示能力、最小化与权限提示，授权/撤销按钮 gate（连接 × Host 支持 × 活动 run × 会话匹配 × 非查询失败态 × 无在途）同源作用于鼠标/键盘/AX 三通道，列表可见条件渲染与 AX 同源（失败占位期间 AX 不发布行与按钮）；撤销不要求仍存在 grant（一次授权消耗后许可与占用可能仍在，revoke 即目标级停止）；动作回执按（会话 + 派出代次）双匹配收敛，切会话/断线使旧代次失效；旧 Host（minor < 27）显示 version_required 占位（未绑定时不卡 loading）。i18n 键约 55 个（en/zh），AX 树覆盖 tab、刷新、目标行与按钮。Desktop 生产依赖红线不变（仍恰好 client/terminal/browser）。

**实际命令**：bash scripts/test.sh desktop（267 passed / 0 failed，含 runtime_shaders）。

**定向回归**：ui/computer.rs 5 个测试，按实际断言——单测覆盖列表查询代次与换会话复位、stale 保留数据并使旧回执失效、动作回执（会话 + 派出代次）双匹配收敛、动作 id 前缀反查；gpui AX 钉板覆盖有目标时行与按钮按实测几何发布、无 client 时按钮禁用且 Press 复核拒绝、查询失败态行与按钮不再发布且迟到 Press 不派出。

**真窗验收**（2026-10-07，证据 /tmp/cu18/evidence.md，截图不入库）：入口与 tab 选中态、无会话空态、no_workspace、no_run、刷新真实往返、断线 stale 横幅与刷新禁用、重连后 stale 消失并重查恢复，均像素/AX 双通道确认。

**遗留缺口**：

- empty 态（活动 run + 0 目标）真窗未覆盖：Global 配置 opencode-go disabled_models 含 glm-5.3-flash，GUI RunStart 被 ADR-055 D4 fail-closed 拒绝（「模型 glm-5.3-flash（provider opencode-go）已被禁用」，协议探针取回完整错误；CLI 同 provider/model 成功，证明 provider 链路正常）。按验证口径未换模型、未改用户配置；该拒绝路径顺带端到端验证通过。no_task / no_workspace / no_run / empty 占位渲染无自动断言（前三者真窗已像素确认；empty 真窗阻塞，且无独立自动覆盖）。
- approve/revoke 真实回执链路未端到端验收：生产侧尚无 agent 路径登记目标（CU-16 落地审批模型，native 操作接入与端到端验收在 CU-20），GUI 对已登记目标授权为 fail-closed 设计；动作路由、gate、回执（会话 + 代次）双匹配收敛由单测与 AX 钉板覆盖，成功动作回执 → 重查收敛链路无真实目标可验。

同步更新：[GUI 设计](../gui-design.md) §8、[Desktop Spec](../spec/desktop.md) §2、[desktop crate Spec](../spec/crates/desktop.md) 模块地图与 §4.7/§7。
