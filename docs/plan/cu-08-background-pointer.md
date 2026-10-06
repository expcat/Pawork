# CU-08 · 后台定向点击、滚动与拖动

> 状态：实现与定向回归完成，真实验收待执行（2026-10-07 控制台锁屏阻塞）。前置：[CU-01](cu-01-background-feasibility.md)、[CU-04](cu-04-window-observation.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

实现已通过无干扰验证的窗口定向指针动作。

## 当前依据与读取范围

当前点击、移动、拖动和滚轮只在 Xvnc 内执行，本机任意控件的后台指针动作不具备已验证实现。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)、[现行设计](../design.md#computer-use-首版)。

## 实施范围

- 基于 CU-01 的应用/动作能力矩阵实现点击、滚动和拖动；只有需要且已证明无干扰才暴露目标内 move。
- 用窗口观测换算坐标，输入前检查尺寸、身份、范围和占用；不移动本机物理指针。
- 按 CU-09 取消并释放按钮；目标未接受或路线需要前台时明确失败。
- 不复制 LCU Linux 的前台激活/XTEST 回退到用户本机，也不以操作后恢复指针掩盖干扰。

## 完成条件与验证

- 目标窗口被遮挡时支持的动作有效，用户鼠标、焦点和输入不中断。
- 拖动取消、窗口重建、越界与审批拒绝有实际拒绝/释放证据。
- 未支持的动作不宣称支持，不自动重试可能已派发动作。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录


实现（2026-10-07，写入集收敛于 computer-use 包与探针，未提交）：`crates/computer-use/src/target.rs` 增 CU-08 契约——`PointerAction`（Click / Drag / Scroll，坐标为观测换算后的窗口点，kind 映射 `NativeActionKind::Pointer`；validate 按观测窗口尺寸复核坐标有限且在 `[0,width) × [0,height)` 边缘不含、滚动增量有限；目标内 move 未证明无干扰、不进入词汇）与 `WindowMove`（全局原点，仅有限值校验、不做屏幕边界钳制——负全局坐标是多显示器合法状态）；`TargetRegistry::consume_window_dispatch` 消费窗口级派发（窗口绑定非租约保持长活，每次消费独立铸单次 `DispatchPermit`，门序与元素消费同族：取消 → 存在性 → Scope → 授权 → 占用 → UserActive → 存活 → spend → 刷新占用）。`crates/computer-use/src/approval.rs` 的 `spend_for_element_dispatch` 泛化更名 `spend_for_dispatch`（凭证纪律不变：一次消费恰放行一次派发尝试，元素与窗口派发共用）。`crates/computer-use/src/macos.rs` 增 `MacosNative::pointer_action`（全族终态拒绝入口：取消 → 台账复查 → 载荷 → 进程实例 → 身份绑定 → 能力闸 Pointer 全族 Unsupported 即 `BackgroundUnsupported`，零事件零 AX 接触，无自动重试，永不回退物理指针或前台激活）与 `MacosNative::move_window`（AXPosition 写路线，CU-01 实测遮挡 AppKit 窗口唯一可落地项：取消 → 派发凭证退役 → 载荷 → 进程实例 → 身份绑定 → 能力闸族半（Chromium 系忽略 AXPosition，零 AX 接触拒绝）→ Accessibility preflight → 存活 + 代际（同一 deadline）→ 窗口解析 → 实读 AXMinimized 最小化半（CU-01 未测最小化移动，Unverified 同拒）→ 取消复查 → armed AXPosition 写；`AxFns` 增 `AXValueCreate` 构造 CGPoint AXValue，返回码复用 `semantic_action_result` 映射，终态 Dispatched / UnknownEffect 与语义动作同口径）。`examples/macos_target_probe.rs` 增 `axpointer`（拒绝路径验收：截图观测 → 矩阵预检 → 观测消费 → 后端权威闸 → 标题 / 前台 / 焦点 / 鼠标 / 剪贴板无干扰复核）与 `axmove`（窗口移动验收：原帧 → 预检 → 窗口派发消费 → 派发 → 帧读回复核 → 还原原位 → 无干扰采样）命令。

实际命令与结果：`bash scripts/test.sh computer-use` 90 项全绿（CU-07 基线 85 + 新增 5：契约层 `pointer_actions_validate_against_observed_bounds` / `window_moves_require_finite_origins` / `window_dispatch_mints_one_permit_per_consume`，macos 层 `pointer_actions_reject_without_dispatching` / `move_window_gates_precede_the_position_seam`——能力拒绝 / 门序 / 凭证一次 / 取消 / 越界拒绝主路径均覆盖，门序拒绝场景 AX 动作缝与遍历缝计数钉零，计数断言合并在单一测试避免并行竞态）；`cargo check -p pawork-computer-use --examples` 通过，探针已构建 `./target/debug/examples/macos_target_probe`。

复审修复（2026-10-07，6.1 sol 审查 2 必须修 + 2 探针建议修，全部落地）：① move_window 最小化半原以 `unwrap_or(false)` 把「AXMinimized 未知」（属性缺失或非布尔）默认成非最小化放行；改为 `move_minimized_gate`：Some(false) 放行、Some(true) 交 require_background 按矩阵拒绝、None 按 ProbeUnavailable（"AXMinimized unknown; cannot prove the window unminimized"）fail-closed——派发必须可证明落在已验证矩阵内，不得默认进矩阵。CU-07 targeted_input 的同形读取是有意语义（缺席=面板非最小化，已有注释），不改。② AXPosition 写的 CGPoint AXValue 原为手工 CFRelease 配对，budget.arm 失败路径（写未发生）会漏释放；改为 `RetainedCfValue` RAII 包装（create-rule：取得后每条退出路径恰一次释放，覆盖 budget 失败与写调用本身），删除手工释放。③ 探针 `axmove` 还原闭包原按派发结果判定（非 Dispatched 不还原），UnknownEffect 时窗口可能已移动却不还原；改为读回帧驱动：读回帧缺失记 "state":"unknown"、原点未动记 unmoved、已移动（与 frame_before 原点差 >0.5px）无论 Dispatched / UnknownEffect 都独立消费还原，rejected 但已移动以 `moved_while_rejected` 如实上报异常。④ `origin_reached` 原漏宽度核对，补 `f[2]==frame_before[2]` 与高度并列。回归新增 2 项：`move_minimized_gate_fails_closed_on_unknown_state`（Some 值原样传递、None 精确错误、纯函数 AX 动作缝计数钉零）与 `retained_cf_value_releases_exactly_once_on_every_path`（CFString 加一 retain 计数 2、包装 drop 后 1、CFString 自释放平衡，恰一次释放可观测）；重跑 `bash scripts/test.sh computer-use` 92 项全绿，`cargo check -p pawork-computer-use --examples` 通过。真实验收仍因控制台锁屏待执行，状态口径不变。

能力要点（CU-01 实测矩阵为准）：拒绝——点击 / 滚动 / 拖动（Pointer）三族后台一律拒绝（AppKit / Electron / Browser 实测 caret 不动、按钮不触发），目标内 move 未证明无干扰不暴露，最小化态窗口移动未实测按 Unverified 同拒，Chromium 系 AXPosition 写被忽略同样拒绝；可用——AppKit 非最小化窗口移动（AXPosition 写，遮挡态实测生效）。

真实验收：未执行——2026-10-07 验收窗口期控制台锁屏（CGSSessionScreenIsLocked=1；窗口服务器对运行中应用呈现零 layer-0 窗口，Terminal 实测 count 0，与 CU-06 / CU-07 锁屏症状一致），CU-01 已明确锁屏未验、不记为支持，锁屏下取得的证据不算本任务验收。探针权限链正常（accessibility:true，screen_recording:true）。待控制台解锁后按序执行：TextEdit `axmove`（移动 → 帧读回 origin_reached=true + 尺寸不变 → 还原原位，全程前台 / 焦点 / 鼠标 / 剪贴板采样不变）→ TextEdit 最小化后 `axmove`（预期能力闸 Unverified 拒绝，或最小化使 AXWindows 失效先按 WindowReplaced 拒，如实记录任一真实终态）→ VS Code `axmove`（Chromium 族半零 AX 接触拒绝，front 不变）→ TextEdit / VS Code / Chrome 专用实例各跑 `axpointer click|drag|scroll`（预期矩阵预检与后端权威闸双 BackgroundUnsupported、标题不变、零派发）；证据 tee 至 /tmp/cu08/（不入库），验收后退出目标应用还原环境。

遗留缺口：真实验收待控制台解锁后执行，通过前 ROADMAP 保持「待开发」、不标 ✅；拖动 / 滚动的无干扰证据与点击同路（CGEventPostToPid 指针事件类），未单独实测——矩阵按整类 Unsupported 拒绝，某族未来被证明需先过 CU-01 口径实测再提升矩阵；生产 tools / Host 接线属 CU-10，不在本任务范围。
