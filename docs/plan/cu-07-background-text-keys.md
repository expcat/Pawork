# CU-07 · 后台定向文本与按键输入

> 状态：自动回归与模拟验收通过，独立 review / 复审 PASS；实现与生产 tools / Host 接线已完成，2026-10-07/08 探针结果保留原范围，真实用户连续输入另列待验。前置：[CU-01](cu-01-background-feasibility.md)、[CU-04](cu-04-window-observation.md)、[CU-05](cu-05-accessibility-tree.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

补齐语义赋值不能覆盖但已证实支持后台输入的文本与按键动作。

## 当前依据与读取范围

虚拟桌面支持 Unicode 文本、组合键和取消释放；本机已采用 CU-01 实测路线，AppKit 支持范围受能力矩阵与目标状态复核约束。CU-10 已完成正式工具 / Host 接线；本轮按授权模拟剩余产品结果链，连续用户输入的真实系统证据单列。已弃用的 AXUIElementPostKeyboardEvent 不作为默认现代路径。

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

真实验收（2026-10-07 晚至 10-08 凌晨执行，macOS 26.6.2 arm64 本机，控制台已解锁；探针路径，证据 tee 至 /tmp/cu20/、不入库；全程用户侧 front / focus 恒为 Edge pid 916、剪贴板 changeCount 不变）：AppKit（TextEdit pid 24371，窗口 cu20keys.txt 全程后台）——insert_text 多 chunk 文本（8 个 🙂 与 6 个 🌍 跨代理对边界）Dispatched，读回完整落地、UTF-16 分块未劈代理对（te-input-text.json；历史叙述曾记首次 TreeChanged 后重新观测成功；本轮复核现存 JSON 仅含成功结果，缺首次拒绝原始记录，不能作为拒绝恢复证据）；key_press return Dispatched，插入点断行、选区 [45,0]→[46,0]（te-key-return.json）；key_press shift+left Dispatched，选区 [46,0]→[45,1] 左扩一字符（te-key-shiftleft.json）；窗口最小化后 insert_text 按最小化闸拒绝（后端实读 AXMinimized 命中矩阵最小化拒）、值不变（te-min-input.json）。Chromium（VS Code pid 25494）——元素级验收同 CU-06 受树深度截断阻塞（UnprovenUnique 不签发，结构性）；Chromium 定向键零 AX 接触拒绝由定向回归钉住。Browser（隔离 Chrome pid 27014）——insert_text 按 Browser TargetedKeys Unsupported 能力闸拒绝、零事件派发（chr-input.json）。验收后目标应用全部退出、环境还原。

真实环境边界：并发无干扰的 session 级单元（用户侧连续输入）仍只保留 CU-01 T5b2 路由级证据与 2026-10-08 front / focus / mouse / clipboard 逐步采样，未新执行 sessiontype；真实连续用户输入待验。VS Code 元素级验收受树深度截断阻塞（结构性）；锁屏场景沿用 CU-01 口径不记为支持。生产 tools / Host 接线已由 CU-10 闭合。

## 本轮模拟验收（2026-10-09）

用户授权人工验收使用模拟。本轮只扩 `crates/tools/src/computer_native.rs` 的既有 Surface 与一条主路径 / 关键拒绝用例，不改生产行为、定向事件计划或能力矩阵。输入经正式 ToolScheduler → NativeComputerTool → NativeHost → TargetRegistry / 授权台账，替身仅提供外部平台结果与存活状态。

| 输入与产品链 | 实际可观察结果 | 证明范围 |
|---|---|---|
| read_tree → find_element → targeted key Left + Shift + Control；平台回 NotFocused | 组合键与修饰参数原样到平台边界；ExecutionFailed、不可重试；零派发计数、零后置捕获，重复旧元素不再进入平台 | 失焦拒绝与参数传播，不证明真实按键释放 |
| 重新读树签发元素，targeted insert_text `😀🌍`；平台回 Partial 2/6 并标记进程退出 | 普通 Text / metadata 保留 `partial 2/6`、ProcessRestarted 和 input_dispatched=true；Host 实际后置复核得到 observation_failure，observation=null，无 Image、无新增捕获 | 部分发送事实与失效目标观测拒绝；2/6 为平台替身提供的计数，不是本次实际 CGEvent 发送计数 |
| 再次调用上述部分发送的元素 | Conflict、不可重试；平台仍共进入两次（一拒绝、一部分发送）且派发一次 | 已消费句柄不自动重发 |
| 既有 `bounded_tree_issues_a_role_match_and_reaches_semantic_and_keyboard` | set_value 后重新读树与 key Dispatched 可达，返回新观测；本轮随十二条工具用例通过 | 成功回执链仍通，不证明目标窗口已接受输入 |

新增用例 `targeted_partial_keeps_platform_counts_and_refusal_is_not_resent`。首次 `01-tools-computer-native.log` 为十二项通过，但当时 Partial 原因写成进程退出、替身进程仍存活且后捕获成功，不能作为修后状态一致性证据。指挥核查后要求原执行对话修复：同一 Surface 在返回 Partial 时调用既有 `kill_process()`，先验失焦拒绝、再验退出后的部分发送，避免虚构进程恢复；原日志保留。最终 `02-tools-computer-native.log` 十二项通过、exit 0，作为本轮验收依据。

历史文件复核：按 Asia/Shanghai 的 mtime，`te-input-text.json`、`te-key-return.json`、`te-key-shiftleft.json` 与 `te-min-input.json` 为 2026-10-07 23:51～23:52，`chr-input.json` 为 10-08 00:17；不把整轮都记作 10-08。成功文本记录为 dispatched 10/10；现存 JSON 不含首次 TreeChanged 拒绝，旧叙述已回写为证据缺失，不用于本轮关闭。

Validated: `cargo test -p pawork-tools --offline --lib computer_native::tests -- --test-threads=8`，12 passed / 85 filtered；格式、diff 与文档链接检查。同版 computer-use 源码与 CU-03 无差异，复用 `/tmp/cu03-20261009/04-computer-use-lib.log` 的 99 项；其中定向事件计划、首组 / 组间复核与 Partial 释放纪律是既有实际投递缝回归，不是本轮 Tool 替身重证。

Targeted regressions: Partial 计数与原因传播、进程退出后的观测失败、失焦拒绝及消费句柄不重发；既有定向文本 / 组合键与取消、目标复核回归保留。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

本轮没有操作用户真实前台、TCC 或目标控件，不证明系统级连续输入隔离、实际 UTF-16 事件分块、物理按键释放或焦点 / 剪贴板无干扰；这些真实证据保留原日期与待验范围。本轮模拟关闭以产品调用与回执传播为边界，不扩展 AppKit 最小化或 Chromium / Browser 能力。

独立 review：`gpt-6.1-sol / max`，对话 `01a12107-cb2f-70b3-b37a-e4d6f58b3cf9`，R1 turn `01a12107-cff5-7cb1-8172-de198e695cbb` PASS；指挥同步 computer-use Spec 历史日期与缺失拒绝证据后，同一 reviewer 的 R2 turn `01a1210e-b801-76a0-8415-397fbf47d52b` PASS。只读核对差异、原始命令 / 日志及版本指纹，未重跑 Cargo；记录 `/tmp/cu07-20261009/REVIEW-R1-PASS.txt`、`REVIEW-R2-PASS.txt`。
