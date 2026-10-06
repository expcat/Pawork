# CU-07 · 后台定向文本与按键输入

> 状态：实现与定向回归完成，真实验收待执行（2026-10-07 控制台锁屏阻塞）。前置：[CU-01](cu-01-background-feasibility.md)、[CU-04](cu-04-window-observation.md)、[CU-05](cu-05-accessibility-tree.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

补齐语义赋值不能覆盖但已证实支持后台输入的文本与按键动作。

## 当前依据与读取范围

虚拟桌面有 Unicode 文本、组合键和取消释放；本机定向输入的无干扰能力尚未证明。已弃用的 AXUIElementPostKeyboardEvent 不能作为默认现代路径。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)、[现行设计](../design.md#computer-use-首版)。

## 实施范围

- 只采用 CU-01 通过的公开系统定向路线，严格绑定目标应用/窗口及输入状态。
- 保留文本、键名、修饰键和取消预算；不采用系统剪贴板粘贴桥或全局事件注入。
- 拒绝不支持的控件/应用，不能临时前置目标窗口或输入后切回焦点。
- 多步按键在取消、错误和目标失效时释放本次按下状态，清楚报告可能已发送的部分。

## 完成条件与验证

- 用户同时在另一应用连续输入时，目标文本/快捷键正确，用户文本不丢失、不混入。
- 取消和超时不会遗留按下键；不自动重发非幂等输入。
- 真实目标结果与用户侧文本记录共同证明无干扰。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

实现（2026-10-07，写入集收敛于 computer-use 包与探针，未提交）：`crates/computer-use/src/target.rs` 增 CU-07 契约——`TargetedKey`（14 个非打印键，可打印字符只能走文本：Unicode 键事件是实测后台路线，字符 keycode 依赖目标键盘布局）、`KeyModifiers`（shift / control / option；Command 刻意不可表示——CU-01 实测菜单快捷键后台不路由）、`TargetedKeyPress`、`TargetedInput`（InsertText 在插入点输入替换非空选区、从不整值替换；KeyPress 带修饰组合，kind 映射 `NativeActionKind::TargetedKeys`；validate：文本 1..=4096 字符无控制字符）、`TargetedOutcome`（Dispatched 派发证明非效果证明 / Partial 释放后如实携带已投计数与原因）与 `TargetError` 第十九变体 `NotFocused`。`crates/computer-use/src/macos.rs` 增 `MacosNative::targeted_input`（CGEventPostToPid 路线）——门序：cancelled → 派发侧台账授权（沿用 CU-06 `DispatchPermit`，一次消费恰放行一次派发尝试）→ 载荷校验 → 进程实例 → 身份绑定 → `require_background` 族闸（Chromium 零 AX 接触拒绝，永不回退 session tap / 全局注入）→ Accessibility preflight → 存活 + 代际（同一 deadline）→ 窗口解析 → 实读 AXMinimized 的最小化闸（AppKit 最小化拒）→ 同边界重走树版本复核 → 路径存在性 → `resolve_element_by_path` → 安全输入框拒绝（`element_is_secure` 与语义写共用）→ 键盘目标复核（窗口 AXMain 与元素 AXFocused 任一非 true 或文本目标无选区即 NotFocused，只读复核绝不 mutate 目标状态）→ 取消复查 → 事件计划投递；投递计划文本按 UTF-16 ≤10 单元分块且不劈代理对、按键 down+up 平衡组、修饰键 flags-changed 累积 / 递减掩码、平衡步骤间查取消与目标存活并 6ms 步进，中止无条件投递释放步骤。`examples/macos_target_probe.rs` 增 `axinput` 验收命令（绑定 → 读树定位 → 矩阵预检 → 签发消费 → 派发 → value_after / 标题 / 版本复核 → 前台 / 焦点 / 鼠标 / 剪贴板无干扰采样），`read_element_text` 增 window_main / element_focused 焦点事实。

实际命令与结果：`bash scripts/test.sh computer-use` 85 项全绿（含新增 4 项：契约层 `targeted_inputs_validate_and_map_to_the_capability_kind` / `targeted_capability_gate_is_the_measured_matrix`，macos 层 `targeted_event_plans_chunk_on_char_boundaries_and_stay_balanced` / `targeted_input_authorization_gates_precede_the_post_seam`——门序拒绝场景投递缝计数钉零，再对本进程 pid 实投四场景验证 Partial 释放纪律与 Dispatched 计数，计数断言合并在单一测试避免并行竞态）；`cargo check -p pawork-computer-use --examples` 通过，探针已构建 `./target/debug/examples/macos_target_probe`。

复审修复（2026-10-07，6.1 sol 审查 2 项必须修，全部落地）：① 多步输入期间未复核绑定——投递循环的步骤间检查从「取消 + 进程存活」扩为「取消 + 进程存活 + 绑定复核」：窗口代际主动证明（`proven_window_cause`）+ 实读 AXMinimized + 实读元素 AXFocused，各次复核独立 250ms 有界（`TARGETED_RECHECK_BUDGET`，不继承整读 10 秒预算——复核守卫每个步骤），任何疑点（窗口已关 / 已最小化 / 元素已失焦 / AX 读失败）fail-closed 停止输入并投递持有的释放、返回 Partial；窗口已关或最小化时继续投递可能落到同 pid 的另一窗口或安全输入框，进程存活证明不了绑定存活。② 释放承诺不结构化——原实现中止路径重创释放事件并以 `unwrap_or(0)` 吞掉创建失败（修饰键已按下后释放创建失败，Partial 仍声称已释放），且首步 ModsDown 创建失败（零派发）也会投递释放事件；改为修饰键释放事件在按下投递前预建持有（`release_specs` 供计划与预建共用同一递减掩码逻辑）：释放承诺由结构保证——清理只投递从不创建、不被取消 / 复核失败 / 后续创建失败阻断，只释放本次实际投递的按下状态（无按下不投任何释放，不干扰目标侧真实修饰键状态）；零事件零按下的创建失败返回明确错误而非 Partial（`post_targeted_plan` 签名相应 Result 化）；创建缝统一 `create_keyboard_event` 并加测试注入（TARGETED_CREATE_FAIL_AT 第 N 次调用失败）。回归扩展（同一测试内串行，计数无竞态）：绑定复核中止的 Partial + 释放、首步按下创建失败零投递零释放明确错误、预建释放创建失败零投递、按下后后续创建失败仍投出预建释放、文本首步失败明确错误与非首步失败保前缀 Partial；重跑 `bash scripts/test.sh computer-use` 85 项全绿，`cargo check -p pawork-computer-use --examples` 通过。真实验收仍因控制台锁屏待执行，状态口径不变。

复审修复第二轮（2026-10-07，6.1 sol 复审 1 项必须修，已落地）：预建释放错误地把 ModsDown 的累积掩码规格传给 `release_specs`（该函数要求独立修饰键掩码）——Shift+Control 时按下规格 (Shift,S)/(Control,S|C) 会算出错误释放 (Control,0)/(Shift,0)，正确应为 (Control,S)/(Shift,0)，正常完成与中止清理都会投递这份错误副本（Control 抬离瞬间系统误认为 Shift 也已抬起）；修法为预建释放直接取计划内匹配 ModsUp 步骤的规格（计划生成时已由 `release_specs` 基于独立掩码算好递减掩码），不再从按下规格重推导，找不到释放步骤按不平衡计划 fail-closed 零投递。回归扩展：双修饰键（Shift+Control+Left）正常完成场景断言实际投出事件的 flags 序列（`TARGETED_POSTED_FLAGS` 逐事件记录）——按下累积掩码 S / S|C、tap 携带 S|C、Control 抬离时 Shift 仍在（S）、Shift 清零收尾；tap-up 从 HIDSystemState 源继承真实键盘状态（fn / numpad，及运行测试者按住的真实修饰键）属 CU-01 实测形状、真实按键与派发残留无法区分，故不对其断言。第三轮微调（同日，复审测试问题）：删除 tap-up「不含虚构组合掩码」断言——用户按住任一修饰键跑测试时合法继承的 flags 会被误判泄漏导致误败；保留按下、tap-down 与释放对的精确断言不变。重跑 `bash scripts/test.sh computer-use` 85 项全绿。真实验收仍因控制台锁屏待执行，状态口径不变。

真实验收：未执行——2026-10-07 验收窗口期控制台锁屏（CGSSessionScreenIsLocked=1；TextEdit 窗口 AX 元素属性全 AttributeUnsupported、标题退化应用名，与 CU-06 记录的锁屏症状一致），CU-01 已明确锁屏未验、不记为支持，锁屏下取得的证据不算本任务验收。探针权限沿责任进程链继承正常（accessibility:true，screen_recording:true）。验收资产已备：/tmp/cu07/te.txt（内容 CU07-BASE）；待控制台解锁后按序执行：TextEdit insert_text 多 chunk 文本（含代理对）落地与焦点事实 → return 按键 → left+shift 组合（选区证据）→ 最小化拒绝（本机此前实测最小化使 TextEdit AXWindows 失效，可能先按 WindowReplaced 拒，如实记录任一真实终态）→ VS Code insert_text 双拒绝（矩阵预检 + 后端权威闸，零 AX 接触，front 不变）→ Chrome 专用实例（--user-data-dir=/tmp/cu07/chrome-profile）insert_text 拒绝；全程前台 / 焦点 / 鼠标 / 剪贴板采样对比，证据 tee 至 /tmp/cu07/（不入库），验收后退出目标应用还原环境。

遗留缺口：真实验收待控制台解锁后执行，通过前 ROADMAP 保持「待开发」、不标 ✅；并发无干扰的 session 级单元（用户侧连续输入）沿用 CU-01 T5b2 路由级证据 + 本轮逐步 front/focus/mouse/clipboard 采样，sessiontype 会向真实前台应用发事件、有干扰用户当前输入的风险，本轮不执行并如实登记；生产 tools / Host 接线属 CU-10，不在本任务范围。
