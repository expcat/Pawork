# CU-05 · 有界无障碍树与元素定位

> 状态：等待人工验收（P2）。前置：[CU-03](cu-03-app-window-discovery.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

为支持的目标应用提供 AX 状态、元素查找和观测绑定的句柄。

## 当前依据与读取范围

当前没有 AX / AT-SPI 树，LCU 的底层树实现未开源；需要通过系统 API 独立实现。

[computer-use Spec](../spec/crates/computer-use.md)、[安全规格](../spec/security.md)。

## 实施范围

- 读取已授权目标的角色、名称、可读状态、范围和支持动作，采用有界深度、节点、文本和读取时间。
- 句柄只对应本次目标实例/树观测；不持久化原生指针，不把节点序号当跨截图稳定身份。
- 查找结果不能歧义选第一个；截断和应用未暴露元素等情况显式返回。
- 遵守敏感字段和不可信内容边界，不为读树顺势获取密码、Token 或其它应用内容。

## 完成条件与验证

- AppKit / Electron 的真实元素结果与窗口可见状态对应。
- 树变化、销毁、歧义和越权句柄都被拒绝，有界树不会无限遍历。
- 明确支持与不支持，不能把空树伪装成应用没有控件。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-07 实现；同日按 6.1 sol 复审意见完成 5 项必须修 + 1 项建议修并重取验收证据；再按第二轮复审意见完成 2 项必须修（时间预算逐调用化、子列表时钟失败释放纪律）并重取验收证据；等待人工验收。

**实现**（写入集：`crates/computer-use/src/target.rs`、`crates/computer-use/src/macos.rs`、`crates/computer-use/examples/macos_target_probe.rs`）：

- 契约层（target.rs）新增有界树类型：`AxTreeBounds`（深度 24 / 节点 4096 / 单串 200 字符 / 整读 10 秒；`AX_TREE_BOUNDS` 常量固定、`validate()` 复核硬上限，初读与消费复核共用同一组边界，宿主不可调）、`AxTreeNode`（子索引 path、深度、角色、可读物名 + 截断标志、enabled/focused、窗口本地帧、动作表、应用上报子数、深度截断标志）、`AxTreeTruncation`（深度 / 节点 / 文本三面截断）、`AxTreeRead`（前序节点流，nodes[0] 恒为窗口根——成功读永不空，根无子是「应用未暴露元素」的肯定答案而非失败伪装）、`ax_tree_revision`（FNV-1a 64 位指纹：同树同边界同版本、发射差异改变输入流；非加密散列，理论碰撞存在，是廉价变更信号而非防篡改证明；帧用窗口本地坐标——纯移动不推进版本，缩放 / 布局变化推进）、`ElementQuery`（角色精确 + 名称大小写不敏感子串，至少一项）、`ElementLookup`（Unique / NoMatch / Ambiguous / UnprovenUnique：截断树下唯一匹配不可证，不签发句柄）与 `lookup_element`。注册表零改动：元素句柄仍只由 AxTree 观测签发、一次性消费，CU-02 既有 `tree_revision` 校验与 CU-16 授权闸原样复用。
- macOS 后端（macos.rs）落地读缝：`MacosNative::read_ax_tree` 门序为台账授权 → 进程实例 → 身份绑定（复用 `verify_instance_owns_bundle`）→ Accessibility preflight（缺权限按 PermissionMissing 先于一切内容读，永不伪装空树）→ 窗口存活 + 代际 → 遍历缝（测试以 `AX_WALK_SYSTEM_CALLS` 计数钉住门序）；`NativeProbe::tree_revision` 由恒 None 桩换为真实实现（属主 pid 定位 → 窗口元素解析 → 同边界重走 → 指纹，任何失败 None fail-closed）。遍历经 ReadBudget 逐调用以剩余预算设 messaging timeout（见下「审查修复」）；角色 / 子列表 / 动作表失败整读失败，可选属性失败降级 None；永不读 AXValue，AXSecureTextField 连名都不读。`resolve_window_element` 复用 CU-03 双模式（`_AXUIElementGetWindow` 精确 id 优先、帧组闭合兜底单候选，歧义 fail-closed）；`ax_window_list` 抽出 retain 元素的 `AxWindowRef`，CU-03 存活路径 `ax_windows` 改为其映射包装、行为不变。AxFns 增 4 个可选符号（CopyAttributeValues / GetAttributeValueCount / CopyActionNames / SetMessagingTimeout，缺失只拒读树、不影响 CU-03 存活）与 `AXUIElementSetAttributeValue`。
- Chromium 族读使能：Electron / Chrome 内核只对设置过 `AXManualAccessibility` 的客户端序列化真实树（VoiceOver 同款读使能，幂等、不改 UI）；每次读树前在应用元素上设置（`ax_enable_full_tree`：属性不支持时如实按当前暴露读，不拒读）。渲染进程异步物化树（真实验收实测约 1–2 分钟），故全新 Chromium 窗口首读可能如实只含窗口铬，后续读收敛到完整树——读失败仍是错误，「此刻只暴露这些」是肯定答案。
- 探针（macos_target_probe.rs）增三命令：`axtree`（未授权拒绝证据 → 授权读树 → 版本 / 截断 / 角色分布 / 节点样本 → 登记 AxTree 观测，图像尺寸取窗口点 1:1）、`axfind <bundle> <role|-> [name|-] [title]`（Unique 签句柄 + 跨 run / 伪造拒绝 + 消费复核版本；NoMatch / Ambiguous / UnprovenUnique 显式返回不签发）、`axstale`（签发 → axsetframe 缩放 → 消费按 TreeChanged 拒绝 → 还原重签 → axclose → 消费按 WindowReplaced 拒绝）。

**复审修复**（2026-10-07，6.1 sol 审查结论 ISSUES，5 项必须修 + 1 项建议修）：

- 安全输入识别改按 subrole：SDK AXRoleConstants.h 把 AXSecureTextField 定义为 subrole（kAXSecureTextFieldSubrole），标准 NSSecureTextField 读出的是 role=AXTextField + subrole=AXSecureTextField，仅比角色永不命中。现对 AXTextField 元素加读 AXSubrole（`ax_secure(role, subrole, read_failed)`），subrole 读取失败保守按敏感处理（不读名、继续遍历）；角色直判保留为非标准暴露的兜底。实测确认：TextEdit 文本区 AXDescription 返回 kAXErrorFailure(-25200) 属真实存在的行为。
- 时间预算贯穿整读：deadline 在门序后、窗口解析前创建，同一 deadline 贯穿 AXManualAccessibility 使能、AXWindows 解析与遍历；每次 AX 调用前把逐元素 messaging timeout 设为剩余预算、返回后复核时钟；符号缺失或设置失败拒读（不再忽略）。修复前第 9 秒进入的节点可带 2 秒逐调用超时超预算成功返回。
- 子列表一致性：已上报正子数的元素，其子列表缺席 / 拉取失败 / 数量不足 / 含空指针一律按结构不一致拒读（ProbeUnavailable）——此前静默当完整分支，查找可能误报 Unique 或确定性 NoMatch。
- 名称读取失败不再伪装不匹配：AXTitle / AXDescription 的真实读取错误（非「属性缺席」）置节点 `name_unreadable` 与读级 `truncation.names`；查找语义精确化——深度 / 节点截断使一切查询不可证，文本截断与名称不可读只使含名称条件的查询不可证（角色单条件查询在结构完整读上仍可签发）。指纹纳入 name_unreadable 与 names 标志。
- 帧读取泄漏修复：`ax_attr_frame` 的 AXSize 错误返回路径现在先释放已 retain 的 AXPosition。
- 文档校正（建议修）：门序测试覆盖表述删去不存在的死窗口用例；安全测试表述改为分类逻辑测试；VS Code 首轮证据数字订正为 vscode_axtree3.json 实际的 179 节点 / 77ms；指纹表述补碰撞限制（见上）。

**第二轮复审修复**（2026-10-07，2 项必须修）：

- 时间预算逐调用化：新增 `ReadBudget`（`set_timeout` + `deadline`，`arm` / `check` / `call`），TreeWalk 的 `deadline` 与 `set_timeout` 字段并入 `budget`。visit() 的每次 AX 调用（role / subrole / AXTitle / AXDescription / enabled / focused / actions / children_count）经 `budget.call` 以该次调用开始时的剩余预算设逐元素 messaging timeout 并在返回后复核时钟；帧读取拆为 AXPosition / AXSize 两次独立 armed 读取（`TreeWalk::read_frame` 取代 `ax_attr_frame`，保留所有权纪律——两次读之间任何失败先释放已 retain 的位置再传播）；visit_children 与 actions 对返回 retained 指针的 copy-rule 调用手动走 arm → 调用 → 存时钟 → 释放 → 传播序列；walk_ax_tree 交付前最终复核时钟。窗口解析同样逐调用：`ax_window_list` 的 app 元素 arm、AXWindows 读取后 check、每窗口 `_AXUIElementGetWindow` id 读与 `ax_window_frame`（签名增 `budget: Option<&ReadBudget>`，AXPosition / AXSize 各自 arm+check；读失败仍 Ok(None) 保持 CU-03 语义）逐次 arm；有 deadline 而 SetMessagingTimeout 符号缺失时按 SymbolsUnavailable 拒绝（不再静默跳过 arming）。修复前 visit 每节点只设一次 timeout、后续调用沿用旧值，children_count 返回后不查时钟，walk_ax_tree 无交付前复核，ax_window_list 窗口解析的 id/位置/尺寸连续读只 arm 一次。
- 子列表时钟失败释放纪律：visit_children 中时钟失败原先发生在 `CFRelease(raw)` 之前，会泄漏已 retain 的子列表数组；现先存时钟结果，所有早退路径先释放非空 raw 再返回时钟错误，成功路径在循环与释放之后复核。
- 预算测试改走真实调用路径（不只测 `remaining_budget` 辅助函数）：`read_budget_arms_each_call_with_the_fresh_remaining_budget`（假 extern "C" fn 记录每次收到的剩余秒数——两次 call 各自 arm 且后者预算不大于前者）、`read_budget_refuses_to_start_a_call_after_the_deadline`（blown deadline 断言假 fn 零调用，arm 先于任何 AX 调用拒绝）、`read_budget_set_failure_refuses_the_read_with_its_cause`（SetFailed 映射：APIDisabled → PermissionMissing / ReadFailed(原始码)，其余码 → ProbeUnavailable）。

**定向回归**：`bash scripts/test.sh computer-use` 全绿（61 项，49 基线 + 12 新）。契约层 3 项：`ax_tree_bounds_are_finite_and_capped`（边界有限且硬上限复核）、`ax_tree_revision_is_deterministic_and_sensitive`（同树同版本；角色 / 名称 / 状态 / 帧 / 动作 / 截断等差异各改变指纹——64 位非加密散列，廉价变更信号而非防篡改证明）、`lookup_never_picks_first_and_flags_unprovable_uniqueness`（歧义不取第一、截断下唯一不可证不签发）。macos 层 9 项：`tree_read_authorization_gates_precede_the_walk_seam`（未授权 / 死进程 / 身份错配先于遍历缝拒绝，计数钉零）、`ax_absent_attribute_codes_are_answers_not_failures`（-25212 / -25205 视为肯定缺席而非失败）、`tree_read_failures_keep_their_cause`（APIDisabled=-25211 → PermissionMissing，其余 → ProbeUnavailable）、`secure_fields_yield_no_content_reads`（安全输入分类逻辑：subrole 判定、识别失败保守按敏感；不读名 / 不读值的执行由 visit 分支结构保证）、`text_truncation_cuts_on_char_boundaries`（文本截断落在字符边界并置标志）、`remaining_budget_reports_only_unexpired_time`（预算用尽后不再发起 AX 调用）、`read_budget_arms_each_call_with_the_fresh_remaining_budget` / `read_budget_refuses_to_start_a_call_after_the_deadline` / `read_budget_set_failure_refuses_the_read_with_its_cause`（ReadBudget 真实调用路径：逐调用新预算、blown deadline 零调用、set 失败映射）。查找测试扩入名称不可读语义：角色单条件查询在 names 截断下仍签发、名称条件查询按 UnprovenUnique 拒绝。

**真实验收**（2026-10-07，macOS 26.6.2 arm64 双显示器；探针为 `cargo build -p pawork-computer-use --example macos_target_probe` 构建的 target/debug/examples/macos_target_probe；Screen Recording 与 Accessibility 经责任进程链授予；原始探针输出存 /tmp/cu05/，不入库；目标经探针不激活启动，全程后台，验收后退出两应用、环境还原）：

- AppKit（TextEdit pid 49691，窗口 30670「note.txt」）：未授权读树按 NotAuthorized 拒绝且先于任何 AX 调用（textedit_axtree.json `read_without_grant`）；授权后读得 12–13 节点、52ms，根 AXWindow「note.txt」帧为窗口本地 [0,0,673,439]，角色分布与可见窗口对应（AXTextArea 获焦、双 AXScrollBar、三枚交通灯 AXButton、AXMenuButton 等），无截断（textedit_axtree.json）。`axfind AXTextArea` 唯一匹配签句柄并消费成功（matches_read_revision=true），跨 run 与伪造句柄分别按 CrossRun / UnknownHandle 拒绝（textedit_axfind_unique.json）；`axfind AXButton` 三匹配按 Ambiguous 拒绝（textedit_axfind_ambiguous.json）；`axfind AXSlider` 零匹配且 tree_truncated=false——「不存在」是完整树上的确定性答案（textedit_axfind_nomatch.json）。`axstale`：窗口缩放 +160/+90 后消费按 TreeChanged 拒绝，还原重签后语义关窗、消费按 WindowReplaced 拒绝（textedit_axstale.json）。
- Electron / Chromium 族（VS Code 1.140.0，pid 49761 → 53101，窗口 30678 / 30748「code.txt」）：全新窗口首读如实只含窗口铬（12–13 节点，vscode_axtree.json / vscode_axtree_fresh.json）；`AXManualAccessibility` 设置后渲染进程异步物化（实测 10 秒轮询内未完成、约 1–2 分钟后收敛），物化后读得 179 节点、77ms，角色含 AXWebArea / AXToolbar / AXTabGroup / AXCheckBox / AXRadioButton / AXPopUpButton，深度截断置位（有界遍历不失控，vscode_axtree3.json；同轮 axfind 各次读取随 UI 加载在 179–200 节点间）。截断树下 `axfind AXWebArea` 唯一候选按 UnprovenUnique 返回、不签发句柄（vscode_axfind_webarea.json）；`axfind AXButton` 31 匹配按 Ambiguous 拒绝（vscode_axfind_ambiguous.json）；`axfind AXSlider` 零匹配但 tree_truncated=true——「未在已读部分找到」不伪装成「不存在」（vscode_axfind_nomatch.json）。

**复审后重取**（2026-10-07，同环境同探针；原始输出存 /tmp/cu05/ 的 *_r2.json）：TextEdit（pid 59203 窗口 31012）读得 13 节点、51ms，truncation.names=true（文本区 AXDescription 的 -25200 如实置标志而非拒读或伪装）；角色单条件 `axfind AXTextArea` 在 names 截断下仍唯一签发并消费成功（matches_read_revision=true）；名称查询 `axfind - note` 两匹配按 Ambiguous 且 tree_truncated=true；axstale 缩放 TreeChanged / 关窗 WindowReplaced 不变。VS Code（pid 60740 窗口 31028；本机 accessibility support 已被首轮使能持久化，首读即物化）181 节点 / 72ms 深度截断，AXWebArea 唯一候选 UnprovenUnique 不签发、AXButton×31 Ambiguous、AXSlider NoMatch 带 tree_truncated=true；严格子列表一致性在两款真实应用上均未误伤。验收后两应用退出。

**复审后重取（第二轮）**（2026-10-07，同环境同探针；原始输出存 /tmp/cu05/ 的 *_r3.json）：TextEdit（pid 67144 窗口 31119「note.txt」）12 节点 / 33ms，truncation.names=true（文本区 AXDescription 的 kAXErrorFailure 如实置标志）；`axfind AXTextArea` 唯一签发并消费成功（matches_read_revision=true）；axstale 缩放 TreeChanged / 还原重签 / 关窗 WindowReplaced 三段语义不变。VS Code（pid 67187 窗口「code.txt」）首读如实只含窗口铬（12 节点 / 28ms），约 4 秒后复读物化 181 节点 / 78ms、深度截断置位；截断树下 `axfind AXWebArea` 唯一候选 UnprovenUnique 不签发（查找执行时树已继续长到 200 节点，证据 vscode_axfind_webarea_r3.json，与复读的 181 节点为两次独立读取）。逐调用预算与严格子列表一致性在两款真实应用上均未误伤。验收后两应用退出。

**遗留缺口**：树变化通知（AXObserver）未接入——版本复核靠消费时同边界重走整树，开销随树规模增长，大树上频繁校验的成本待 CU-06+ 实测再定。Chromium 族首读物化延迟无系统信号可等待，只能如实返回当前暴露部分（已文档化）。生产 tools / Host 接线与 GUI 观测查询属 CU-10 / CU-17，本任务未接入生产路径。
