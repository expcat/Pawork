# CU-09 · 目标占用、用户接管与后台生命周期

> 状态：等待人工验收。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

把现有全局串行改成明确目标归属，保证并发冲突、取消和后台生命周期可解释。

## 当前依据与读取范围

ComputerTool 使用进程级 OnceLock，物理桌面与 generation 全局共享；串行锁不等于 Run 隔离。关闭 Desktop 已不取消进入 Core 的 Run。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)、[app Spec](../spec/crates/app.md)、[GUI 连接流程](../spec/flows.md)。

## 实施范围

- 按桌面/应用等实际共享边界绑定 workspace/run 所有权，首版明确拒绝冲突，不引入容器池或完整调度框架。
- 用户操作同一目标时优先让出并暂停后续派发；前台状态或目标使用意图不确定时采用安全暂停，不竞争。
- 停止、取消、授权撤销、超时、Host 连接/任务结束使句柄失效并释放本次输入与占用。
- 正常后台 Run 不因 GUI 隐藏、切任务或关闭窗口取消；Host 生命周期仍沿用正式 pawork，不新增 Core daemon。
- 后台期间无需 Agent 抢占目标前台或持续获取系统剪贴板，异常状态真实通知。

## 完成条件与验证

- 两个 Run 的冲突可见，不能互相使用观测或把全局锁当隔离完成。
- 用户接管后不继续派发；取消后无残留键、按钮、占用或错误归属回执。
- GUI 生命周期与 Run 生命周期保持现有合同，恢复只重放状态不重做动作。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-07 实现；自动定向回归全绿；真实隔离桌面与 GUI 后台生命周期人工验收待执行。

**实现**（写入集：`crates/computer-use/src/lib.rs`、`crates/computer-use/src/target.rs`、`crates/computer-use/src/approval.rs`、`crates/tools/src/computer.rs`，同批更新两包 Spec）：

- 隔离桌面路径（lib.rs）把进程级 `DESKTOP_LOCK` 全局串行锁与全局 `DESKTOP_GENERATION` 替换为按实际共享边界绑定的桌面会话所有权：`DesktopSession`（operation 串行化同桌面整操作 + state 短临界区持有 owner / generation / lease），`Computer::isolated()` 的全部实例经进程级 OnceLock 共享一个会话槽（一个部署只有一个桌面、单 viewer），`new(backend)` 自持私有会话。所有权闸先于一切锁等待与后端访问：非拥有 Run 的任何动作（含 status）立即返回 `Error::Conflict(owner)`、零后端调用，不存在排队后拒绝或跨 Run 共享观测。取消 / 超时（cancelled 闭包命中或 backend 输入中途取消）即时释放占用并作废待用观测，键鼠平衡仍由 RFB backend 保证；占用者空闲超过 `OWNERSHIP_TTL`（60 秒，与观测租约同寿——所有权永不长于可用观测）自动失拥有；新增 `Computer::release(scope)` 宿主 Run 终止钩子（非拥有 scope 调用为无害 no-op）。
- 契约层（target.rs）新增目标占用与用户接管：`TargetRegistry` 增占用台账（anchor → 拥有 scope + 活跃时间）与接管暂停集；占用边界为应用进程（`TargetIdentity::occupancy_anchor()`：应用本体，或网站目标所在浏览器——输入派发到浏览器进程，不同 origin 同浏览器同样冲突）。bind_window 认领占用，begin_observation / validate_window / consume_observation / consume_element 全链刷新或按第十六变体 `TargetOccupied` 拒绝（校验顺序插在 Scope 之后）；占用者空闲 60 秒自动失效。`pause_dispatch(target)` / `resume_dispatch(target)` 承接宿主用户接管信号：暂停期间两个派发闸在授权检查后、任何 probe 之前按第十七变体 `UserActive` 让出且不消耗租约（观测保持可用，接管结束后重观测再派发）；宿主无法判定用户意图时保持暂停（fail-closed），不与用户竞争。`release_scope(scope)` 在 Run 终止（停止 / 取消 / 超时 / Host 断开 / 任务结束）作废该 scope 全部句柄（后续使用 UnknownHandle）并释放占用；`revoke` / `revoke_scope` 保留 CU-16 的 NotAuthorized 阻断语义与已消费历史，额外释放占用。
- tools 接线层（computer.rs）：`Error::Conflict` 映射 `ToolErrorKind::Conflict`（错误消息携带拥有者 scope，冲突对模型可见）；descriptor 补充单占用语义说明。ComputerTool 进程共享 OnceLock 结构不变（它就是共享边界的载体），改由 Computer 层执行所有权。
- GUI 生命周期合同不变：正常后台 Run 不因 GUI 隐藏 / 切任务 / 关窗取消、resume 只重放不重做，由既有 app 回归覆盖（`computer_approval_image_persistence_and_resume_do_not_repeat_input` 等）；Host 生命周期仍用正式 pawork，未新增 Core daemon。后台期间不抢占目标前台、不持续读系统剪贴板为既有红线（RFB 丢弃 clipboard 消息、macos 后端永不激活），本任务未触碰。

**定向回归**：`bash scripts/test.sh computer-use tools app` 全绿（exit 0，63s；computer-use 68 项 / tools 81 项 / app 275+ 项，一次 Cargo 调用含 ui-fixture）。新增八项：tools `another_run_conflict_is_visible_and_never_touches_the_backend`（第二个 Run 的 Conflict 错误可见且后端零调用、拥有者观测照常派发）；lib.rs `second_run_gets_a_visible_conflict_without_touching_the_backend`（冲突可见 + 零后端调用 + 拥有者观测照常派发）、`cancellation_releases_ownership_and_invalidates_the_observation`（取消后无占用 / 无可派发租约残留，新 Run 立即接管）、`release_and_idle_expiry_free_the_desktop_for_the_next_run`（非拥有 scope 不能偷释放、拥有者释放生效、空闲超 TTL 失拥有且孤儿观测不可派发）；target.rs `second_run_conflicts_on_the_same_application_until_released`（同应用冲突、不同应用不冲突、网站按浏览器进程冲突、release_scope 作废句柄并释放占用）、`idle_occupancy_lapses_for_the_next_run`、`user_takeover_pauses_dispatch_without_consuming`（暂停期间派发 UserActive 且租约不消耗、观测保持可用、恢复后继续派发）、`revocation_releases_occupancy_while_leases_block`。既有跨 Run 观测测试按新语义订正（跨 Run 调用先撞所有权闸 → Conflict，仍不可互用观测）。

**真实 / 人工结果**：隔离桌面 Docker（127.0.0.1:5905）本机未运行，真实桌面上两 Run 冲突 / 取消释放 / 后台存活待人工验收（验收时先起 desktop/compose.yaml，再以两个不同 run scope 走 `examples/isolated_probe.rs` 或真实 Host 双会话验证 Conflict 与释放）；原生目标路径为契约层实现，生产接线与真实用户接管信号源属 CU-10 / CU-17 / CU-18，届时随真实目标状态一并人工验收。RFB wire 级取消 / 键平衡由既有 rfb 回归覆盖（真实监听线程）。

**复审修复**（2026-10-07，6.1 sol 审查结论 ISSUES，5 项必须修，写入集增 `crates/app/src/services/run.rs` 并同批回写 computer-use / tools / app 三包 Spec）：

- release 后在途操作不得重新认领：`abandon`（release 的状态侧实现）推进 generation；截图存租约与输入派发前的复核改用新 `verify_owned`——只验证原所有权（本 scope 通过并刷新活跃、他人 Conflict、空槽 Cancelled），禁止重新认领。修复前截图在 `backend.capture` 期间被 release 后会在存租约处 `claim_state` 重新认领空槽；输入存在「取租约 → release → 重新认领 → generation 通过 → 派发」交错。
- 生产 Run 终态释放占用：`ComputerTool` 的进程级共享会话提为模块级 `SHARED_COMPUTER`，scope 构造提为 `computer_scope` 与 execute 同源，新增 `ComputerTool::release_shared(workspace_id, run_id)`；app `chat_turn_with_run_id` 在 run 终态（完成 / 取消 / 失败，含子代理 Run——child 同走该入口）统一调用，GUI 断连不取消 Run 亦不触发。修复前 Run 截图后正常结束，共享 Computer 保留占用，后续 Run 在 TTL 内撞 Conflict。
- 后端错误分支不再绕过取消释放：认领后全部 backend 调用（permissions / capture / desktop / input）统一经 `run_backend` 包装——Err 且 `Error::Cancelled` 或 `cancelled()` 时先 release 再返回原错误。修复前取消发生在 backend 调用期间且后端返回 Err 时 `?` 直接退出，owner 残留。
- `require_authorized` 补所有权闸：授权检查后、任何 probe（含网站 origin probe）之前过 `occupancy_conflict`；预检与 `validate_window` 均不刷新占用活跃（刷新只在 bind 认领与 begin / consume 成功使用）。修复前 A 占用过期、B 认领后，A 旧窗口句柄仍过预检，网站路径继续 origin probe，Host 可捕获内容到 begin_observation 才被拒。
- 失败绑定不再占用 60 秒：`bind_window` 改为先 `occupancy_conflict` 只读检查，存活与 origin 校验全部通过后再 `touch_occupancy` 提交认领（失去调用方的 `claim_occupancy` 删除）。修复前 ProcessRestarted / SiteChanged 的失败绑定已在台账留下 60 秒占用。

同批：测试 Fake 的零后端计数从只统计 permissions 扩到全部 Backend 方法（lib.rs 与 tools 两处）；新增回归 lib.rs `release_during_screenshot_prevents_reclaiming_the_desktop`、`release_after_the_lease_is_taken_prevents_input_dispatch`、`backend_error_during_cancellation_releases_ownership`，target.rs `pre_flight_rejects_a_taken_over_application_before_any_probe_call`、`failed_binding_leaves_no_occupancy_behind`，tools `release_shared_frees_exactly_the_scope_execute_claims`。复审后 `bash scripts/test.sh computer-use tools app` 全绿（exit 0，105s；computer-use 73 项 / tools 82 项 / app 275+2+1 项）。

**第二轮复审修复**（2026-10-07，2 项必须修）：

- release 停止已进入 backend 的输入：`backend.input` 的取消谓词从裸外部 `cancelled` 改为「外部取消 OR 所有权撤销」——撤销谓词锁 state 检查 owner 仍为本 scope 且 generation 仍为派发代际（锁中毒 fail-closed 视为已撤销）。RFB backend 本就逐事件（click / drag 步 / scroll 步 / type 字符）检查取消谓词，release 落地后剩余文本字符与拖动事件即停、按 Cancelled 返回，键鼠平衡由 backend 收尾保证。输入派发代际改由认领复核块显式返回（`dispatch_generation`）。回归 `release_during_input_stops_the_remaining_events`（Fake 的 TypeText 逐字符记录并逐字符检查谓词：首字符已派出、release 后第二字符不再发送、无占用与租约残留）。
- 同 scope 重认领时在途截图不交付旧代际观测：截图存租约的临界区在 `verify_owned` 之后新增原代际复核——`generation != state.generation` 即 Stale。交错「截图开始 → release 推进代际 → 同 scope 排队调用重新认领 → 原截图完成」不再成功返回一个下次必 Stale 的观测。回归 `screenshot_after_release_and_same_scope_reclaim_delivers_no_stale_observation`（capture 期间 release + 同 scope 重认领：按 Stale 拒绝、不存租约）。

复审后 `bash scripts/test.sh computer-use tools app` 全绿（computer-use 75 项 / tools 82 项 / app 275+2+1 项）。

**遗留缺口**：隔离桌面的 Run 终态释放已接线 app（`ComputerTool::release_shared`），Host 进程被杀死时仍靠占用 TTL 60 秒自愈；契约层 `release_scope` / 用户接管信号源（GUI 观测用户操作目标应用）的生产接线属 CU-10 / CU-18；冲突等待策略首版为明确拒绝，无排队或抢占。
