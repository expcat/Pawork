# CU-08 · 后台定向点击、滚动与拖动

> 状态：已实现；自动回归与模拟验收通过；独立 Sol max review PASS；本机 Pointer 整类拒绝，AppKit 非最小化窗口移动可用。真实系统权限与持续无干扰复验待验。前置：[CU-01](cu-01-background-feasibility.md)、[CU-04](cu-04-window-observation.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

实现已通过无干扰验证的窗口定向指针动作。

## 当前依据与读取范围

点击、目标内指针移动、拖动和滚轮由隔离 Xvnc 路线支持。本机 Pointer 整类按 CU-01 实测能力矩阵拒绝；唯一支持的窗口移动是 AppKit 非最小化窗口 AXPosition 写。CU-10 已接正式工具 / Host，本轮只核既有产品效果与拒绝证据，不按标题提升能力矩阵。

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


实现（2026-10-07，历史写入集收敛于 computer-use 包与探针，当时未提交，现已进入基线）：`crates/computer-use/src/target.rs` 增 CU-08 契约——`PointerAction`（Click / Drag / Scroll，坐标为观测换算后的窗口点，kind 映射 `NativeActionKind::Pointer`；validate 按观测窗口尺寸复核坐标有限且在 `[0,width) × [0,height)` 边缘不含、滚动增量有限；目标内 move 未证明无干扰、不进入词汇）与 `WindowMove`（全局原点，仅有限值校验、不做屏幕边界钳制——负全局坐标是多显示器合法状态）；`TargetRegistry::consume_window_dispatch` 消费窗口级派发（窗口绑定非租约保持长活，每次消费独立铸单次 `DispatchPermit`，门序与元素消费同族：取消 → 存在性 → Scope → 授权 → 占用 → UserActive → 存活 → spend → 刷新占用）。`crates/computer-use/src/approval.rs` 的 `spend_for_element_dispatch` 泛化更名 `spend_for_dispatch`（凭证纪律不变：一次消费恰放行一次派发尝试，元素与窗口派发共用）。`crates/computer-use/src/macos.rs` 增 `MacosNative::pointer_action`（全族终态拒绝入口：取消 → 台账复查 → 载荷 → 进程实例 → 身份绑定 → 能力闸 Pointer 全族 Unsupported 即 `BackgroundUnsupported`，零事件零 AX 接触，无自动重试，永不回退物理指针或前台激活）与 `MacosNative::move_window`（AXPosition 写路线，CU-01 实测遮挡 AppKit 窗口唯一可落地项：取消 → 派发凭证退役 → 载荷 → 进程实例 → 身份绑定 → 能力闸族半（Chromium 系忽略 AXPosition，零 AX 接触拒绝）→ Accessibility preflight → 存活 + 代际（同一 deadline）→ 窗口解析 → 实读 AXMinimized 最小化半（CU-01 未测最小化移动，Unverified 同拒）→ 取消复查 → armed AXPosition 写；`AxFns` 增 `AXValueCreate` 构造 CGPoint AXValue，返回码复用 `semantic_action_result` 映射，终态 Dispatched / UnknownEffect 与语义动作同口径）。`examples/macos_target_probe.rs` 增 `axpointer`（拒绝路径验收：截图观测 → 矩阵预检 → 观测消费 → 后端权威闸 → 标题 / 前台 / 焦点 / 鼠标 / 剪贴板无干扰复核）与 `axmove`（窗口移动验收：原帧 → 预检 → 窗口派发消费 → 派发 → 帧读回复核 → 还原原位 → 无干扰采样）命令。

实际命令与结果：`bash scripts/test.sh computer-use` 90 项全绿（CU-07 基线 85 + 新增 5：契约层 `pointer_actions_validate_against_observed_bounds` / `window_moves_require_finite_origins` / `window_dispatch_mints_one_permit_per_consume`，macos 层 `pointer_actions_reject_without_dispatching` / `move_window_gates_precede_the_position_seam`——能力拒绝 / 门序 / 凭证一次 / 取消 / 越界拒绝主路径均覆盖，门序拒绝场景 AX 动作缝与遍历缝计数钉零，计数断言合并在单一测试避免并行竞态）；`cargo check -p pawork-computer-use --examples` 通过，探针已构建 `./target/debug/examples/macos_target_probe`。

复审修复（2026-10-07，6.1 sol 审查 2 必须修 + 2 探针建议修，全部落地）：① move_window 最小化半原以 `unwrap_or(false)` 把「AXMinimized 未知」（属性缺失或非布尔）默认成非最小化放行；改为 `move_minimized_gate`：Some(false) 放行、Some(true) 交 require_background 按矩阵拒绝、None 按 ProbeUnavailable（"AXMinimized unknown; cannot prove the window unminimized"）fail-closed——派发必须可证明落在已验证矩阵内，不得默认进矩阵。CU-07 targeted_input 的同形读取是有意语义（缺席=面板非最小化，已有注释），不改。② AXPosition 写的 CGPoint AXValue 原为手工 CFRelease 配对，budget.arm 失败路径（写未发生）会漏释放；改为 `RetainedCfValue` RAII 包装（create-rule：取得后每条退出路径恰一次释放，覆盖 budget 失败与写调用本身），删除手工释放。③ 探针 `axmove` 还原闭包原按派发结果判定（非 Dispatched 不还原），UnknownEffect 时窗口可能已移动却不还原；改为读回帧驱动：读回帧缺失记 "state":"unknown"、原点未动记 unmoved、已移动（与 frame_before 原点差 >0.5px）无论 Dispatched / UnknownEffect 都独立消费还原，rejected 但已移动以 `moved_while_rejected` 如实上报异常。④ `origin_reached` 原漏宽度核对，补 `f[2]==frame_before[2]` 与高度并列。回归新增 2 项：`move_minimized_gate_fails_closed_on_unknown_state`（Some 值原样传递、None 精确错误、纯函数 AX 动作缝计数钉零）与 `retained_cf_value_releases_exactly_once_on_every_path`（CFString 加一 retain 计数 2、包装 drop 后 1、CFString 自释放平衡，恰一次释放可观测）；重跑 `bash scripts/test.sh computer-use` 92 项全绿，`cargo check -p pawork-computer-use --examples` 通过。当时真实验收因控制台锁屏待执行；后续探针记录与本轮模拟结果见下文。

能力要点（CU-01 实测矩阵为准）：拒绝——点击 / 滚动 / 拖动（Pointer）三族后台一律拒绝（AppKit / Electron / Browser 实测 caret 不动、按钮不触发），目标内 move 未证明无干扰不暴露，最小化态窗口移动未实测按 Unverified 同拒，Chromium 系 AXPosition 写被忽略同样拒绝；可用——AppKit 非最小化窗口移动（AXPosition 写，遮挡态实测生效）。

历史真实探针（2026-10-07 晚至 10-08 凌晨，macOS 26.6.2 arm64，控制台已解锁；证据 /tmp/cu20/ 不入库，时间按 Asia/Shanghai 文件 mtime 核对）：TextEdit 的 te-move.json 为 10-07 23:52:44，AXPosition 派发使窗口从 (470,132) 移至 (900,300)，尺寸 673×439 不变，随后还原。此文件 front / focus 前后为 Codex pid 24196、剪贴板不变，mouse_delta 约 (211.4,150.3)；它证明目标位置改变与还原，不能单凭该采样证明移动没有干扰鼠标。

TextEdit 的 Pointer click / drag / scroll 文件为 10-07 23:53，各自按整类能力闸拒绝、标题不变；VS Code 的 move 与 Pointer click 为 10-07 23:59，族闸拒绝；Chrome 的 move 与 Pointer click / scroll 为 10-08 00:17，族闸拒绝。上述拒绝文件 front / focus 为 Edge pid 916，剪贴板不变；用户鼠标采样有移动不等于目标派发。TextEdit 最小化移动拒绝文件 te-min-move.json 为 10-08 00:51，帧不变、restore=unmoved。没有 VS Code drag / scroll 或 Chrome drag 的原始探针文件，不把缺失组合写成实测成功。

真实环境待验：系统 Accessibility / Screen Recording 授权、连续用户输入下的无干扰复验、真实 AXPosition 超时及多显示器负原点效果未在本轮重放；锁屏不记为支持。未实测的族 × Pointer 动作组合继续按整类 Unsupported 拒绝，未来提升能力仍须 CU-01 实证。生产 tools / Host 接线已由 CU-10 完成。


## 本轮模拟验收（2026-10-09）

Grok xhigh 执行对话 01a12111-7ca7-7bb1-b18f-aca958855a03 核当前 main 基线 fbd92de0：源码 no-op，不新增测试、不运行 Cargo。证据目录 /tmp/cu08-20261009/ 保留完整交付、原始命令与退出码、源码和日志指纹。

复用同版正式链回归：Tool JSON → Scheduler → NativeHost → TargetRegistry → 外部平台替身。多窗口用例选择窗口 22 移至 (12,8)，只改变该窗原点，窗口 11 仍 (0,0)，返回新 observation 与窗口 22 的同窗 JPEG。正式 Pointer 请求仅含 action/window，由 Host 检查 scope 和句柄后在能力闸拒绝，不消费派发凭证、不调用平台、不回退全局输入。负原点 (-1300,200) 仅由库契约接受和 Host 原样传递源码证明，本轮未经正式工具链重跑负坐标。

Once 动作后授权耗尽仍保留派发事实，无后置图；ForRun 后捕获失败只调用一次移动，不重试。UnknownEffect 共用 AX 返回码映射和 Host 派发终态；本轮正式链已验证语义 UnknownEffect，但未单独注入窗口移动 UnknownEffect，不声称位置不变。隔离 RFB 的拖动中途取消有按钮释放回归；本机 Pointer 整类拒绝没有按下，二者分开记。最小化未知、族闸、scope、撤销、接管、进程及窗口代际拒绝复用已有定向锚点。

Validated: git diff --check、相对文档链接和同版日志/源码指纹核对；不重跑 Cargo。工具日志 /tmp/cu07-20261009/02-tools-computer-native.log（12 passed / 85 filtered / EXIT 0），computer-use 日志 /tmp/cu03-20261009/04-computer-use-lib.log（99 passed / EXIT 0）复用。
Targeted regressions: 复用多窗口移动/新 JPEG、Pointer 零派发、Once/捕获失败不重试、负原点契约、最小化未知拒绝、AX 返回码、RFB 取消拖动；平台替身不证明真实 TCC 或鼠标像素效果。
Full workspace gate: NOT RUN。

独立 review：gpt-6.1-sol / max 对话 01a12125-5613-7053-8fdc-bd790c9616c8、turn 01a12125-5a49-7e43-ba7c-8f4bd4535574 返回 PASS，无必须修问题；指挥提交本项状态与历史证据纠正。
