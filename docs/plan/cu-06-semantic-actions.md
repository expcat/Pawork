# CU-06 · 后台 AX 语义动作

> 状态：实现与定向回归完成，真实验收待执行（2026-10-07 控制台锁屏阻塞）。前置：[CU-04](cu-04-window-observation.md)、[CU-05](cu-05-accessibility-tree.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

优先用目标元素的语义动作实现后台点击、选择和赋值。

## 当前依据与读取范围

现有 computer 工具只派发坐标和键鼠输入；AX 语义动作只对支持该动作的控件成立。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)、[policy Spec](../spec/crates/policy.md)。

## 实施范围

- 仅暴露 CU-01 已证明无干扰的 press / select / set-value 等动作，输入绑定新鲜元素句柄。
- 调用前核对目标、审批、占用与当前能力，不通过激活窗口绕过应用限制。
- 区分明确拒绝、已派发和执行结果未知；AX 超时可能已有副作用，不盲目重试。
- 操作后用新观测和目标结果核对；应用触发焦点或剪贴板副作用时撤回该后台能力声明。

## 完成条件与验证

- 语义动作能在后台改变目标控件，用户键鼠、焦点和剪贴板不受影响。
- 不支持、过期、拒绝与取消均正确反馈；不能失败后回退全局输入。
- 相关既有安全/观测回归保留，只补当前行为的必要主路径及关键拒绝证据。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

实现（2026-10-07，写入集收敛于 computer-use 包与探针，未提交）：`crates/computer-use/src/target.rs` 增 `SemanticAction`（Press / SetValue / InsertText——仅 CU-01 实测无干扰的动作，未实测独立 select 动作故不暴露，InsertText 即实测的「选择」原语）、`SemanticOutcome`（Dispatched 派发非效果证明 / UnknownEffect 超时效果未知不盲目重试）、`MAX_SEMANTIC_TEXT_CHARS` = 4096（按字符计，空串合法即清空）与 `TargetError` 第十八变体 `ElementUnsupported`（元素自身肯定拒绝，区别于能力闸前置拒绝）；`crates/computer-use/src/macos.rs` 增 `MacosNative::semantic_action`——门序为台账授权 → 载荷校验 → 进程实例 → 身份绑定 → `require_background` 能力闸（先于一切 AX 接触，Chromium 语义写在此零接触拒绝）→ Accessibility preflight → 存活 + 代际 → 同边界重走复核树版本（不一致 TreeChanged）→ 路径存在性 → `resolve_element_by_path` 逐级 armed 有限切片重解析（缺序 TreeChanged、null 洞 ProbeUnavailable）→ 写类 `refuse_secure_write`（安全输入框 Invalid 拒，与读侧对称）→ armed 单次 AX 调用（不可回退点，返回码单独定终态），同一 deadline（AX_TREE_BOUNDS.max_read）贯穿全程；`examples/macos_target_probe.rs` 增 `axact` 全流程验收命令（绑定 → 读树 → 唯一定位 → 矩阵预检 → 签发消费 → 派发 → AXValue / 选区 / 窗口标题读回复核 → 前台 / 焦点 / 鼠标 / 剪贴板无干扰采样；读回为探针专用独立事实取证，产品路径永不读 AXValue），`bind_window` 支持同 bundle 多进程实例按标题落到持有匹配窗口的实例。

实际命令与结果：`bash scripts/test.sh computer-use` 79 项全绿（含新增 4 项：契约层 `semantic_actions_validate_and_map_to_capability_kinds` / `semantic_capability_gate_is_the_matrix_and_minimized_invariant`，macos 层 `semantic_action_authorization_gates_precede_the_action_seam`（动作缝计数钉零）/ `semantic_action_result_maps_ax_codes_to_terminal_states`）；`cargo build -p pawork-computer-use --example macos_target_probe` 通过（仅基线既有警告）。

复审修复（2026-10-07，6.1 sol 审查 4 项必须修，全部落地）：① Once 消费后派发必然误拒——`TargetAuthorizer` 增单次派发凭证 `DispatchPermit`：`consume_element` 经 `spend_for_element_dispatch` 消费授权并铸进程级命名空间（pid+纳秒+序号）的唯一凭证随 `ValidatedElement` 携带，`semantic_action` 以 `consume_dispatch_permit` 原子退役凭证（一次尝试即失效，克隆消费结果无法二次派发；观测 spend 不产凭证），revoke / revoke_scope 同清未用凭证，合法 Once 动作不再误报 NotAuthorized，撤销在途派发仍阻断。② 耗时复核期间取消仍派发——`semantic_action` 增 `cancelled` 谓词，入口与不可回退 AX 调用前各查一次；调用发起后保留 Dispatched / UnknownEffect 真实终态。③ 存活证明未纳入 deadline——新增 `proven_window_cause`（Option<Instant>），派发路径传入同一 deadline，存活读取超时按 ProbeUnavailable 超时原因而非 WindowReplaced；CU-03 既有调用方保持无界纪律不变。④ 路径解析超时分支漏释放 CF 数组——clock 错误传播前先 CFRelease。另删除零字节残留 /tmp/cu06/textedit_setvalue.json（非验收证据）。重跑 `bash scripts/test.sh computer-use` 81 项全绿（新增 approval 层 Once 派发协调、macos 层泄漏失败路径 2 项回归；门序测试扩展 Once 消费后派发过闸 / 撤销阻断 / 取消优先，计数钉零）；探针随签名适配，`cargo build -p pawork-computer-use --example macos_target_probe` 通过。第二轮复审（同日）指出 ① 初版的 spent_once 标记 + `check_dispatch` 未绑定单次派发（同一克隆消费结果可重复派发、观测 spend 也可充当元素派发许可），已收紧为上述凭证机制；重跑 `bash scripts/test.sh computer-use` 81 项全绿（approval 层 `dispatch_permit_is_bound_to_one_consume_and_one_attempt` 替换标记回归，门序测试增同一凭证重复派发拒绝），探针构建通过。复审末项（同日）：`TargetAuthorizer` 仍派生 Clone——`dispatch_permits` 随账本深拷贝，同一消费结果可经原账本与副本各派发一次、原账本 revoke / revoke_scope 不清副本凭证；已移除 Clone 派生（无调用方依赖），并以编译期断言钉住账本复制路径不可行（Clone 类型触发 E0283 歧义，经 /tmp 临时 fixture 双向验证，不入库）。重跑 `bash scripts/test.sh computer-use` 81 项全绿，探针构建通过。真实验收仍待控制台解锁，ROADMAP 保持「待开发」。

真实验收：未执行——2026-10-07 验收窗口期控制台锁屏（CGSSessionScreenIsLocked=1），锁屏下窗口服务器对所有应用呈现零 layer-0 窗口，TextEdit 与隔离 profile Chrome 均无法取得真实窗口；CU-01 已明确锁屏未验、不记为支持，锁屏下取得的证据不算本任务验收。探针权限沿责任进程链继承正常（accessibility:true，screen_recording:true）。验收资产已备：/tmp/cu06/note.txt、page1.html（按钮 CU06-BTN 置标题 CU06-PRESSED、链接 CU06-LINK 导航 page2、textarea 初值 CU06-TA-INIT、静态文本）、page2.html（按钮 CU06-BTN2），/tmp/cu06chrome 隔离 profile；环境已还原，无残留进程。待控制台解锁后按序执行：TextEdit set_value / insert_text 生效读回与 press 拒绝（AppKit AxPress Unverified）、VS Code set_value 双拒绝（矩阵预检 + 后端权威闸，零 AX 接触，值不变）、Chrome 按钮按压标题变化 / 链接按压导航 / AXStaticText 按压 ElementUnsupported / set_value Unverified 拒绝、最小化变体如实记录；全程前台 / 焦点 / 鼠标 / 剪贴板采样对比，证据 tee 至 /tmp/cu06/（不入库），验收后退出目标应用还原环境。

遗留缺口：真实验收待控制台解锁后执行，通过前 ROADMAP 保持「待开发」、不标 ✅；生产 tools / Host 接线属 CU-10，不在本任务范围。
