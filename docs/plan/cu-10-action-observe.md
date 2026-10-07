# CU-10 · 动作与新观测的 Host 闭环

> 状态：隔离路径实现与定向回归完成、等待人工验收；native 接线待开发（P3）。前置：[CU-04](cu-04-window-observation.md)、[CU-06](cu-06-semantic-actions.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

减少单独截图/动作消耗的回合，同时诚实区分输入派发和 UI 接受。

## 当前依据与读取范围

当前输入后由模型再调用 screenshot；输入成功只证明派发。ToolExecutionCompleted / MessageCommitted 与图片已经能进入持久消息并重放。

[tools Spec](../spec/crates/tools.md)、[engine Spec](../spec/crates/engine.md)、[app Spec](../spec/crates/app.md)、[持久化流程](../spec/flows.md)。

## 实施范围

- 一个有界工具调用执行一个动作并返回新观测，保留动作前观测的单次消费与 scope 校验。
- 结果记录输入是否派发、操作状态、新观测或观测失败；后置捕获失败不能抹掉已经发生的输入。
- 沿用现有工具消息、事件和 Provider 图片编码，不另建 Agent loop 或持久事件旁路。
- 取消/恢复和出错时不重执行动作，不引入无界批量命令或 JS REPL。

## 完成条件与验证

- 用户可通过新观测核对效果，动作状态不把派发成功等同任务完成。
- 输入已发生但截图失败时，结果仍保留真实状态且不自动重试。
- 工具结果和图像能持久化、重放，既有审批/恢复回归仍覆盖真实链路。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

实现（2026-10-07，写入集收敛于 computer-use 与 tools 两包，未提交）：隔离桌面主干闭环——`crates/computer-use/src/lib.rs` 把 screenshot 动作的捕获路径提取为 `Computer::capture_observation`（generation 推进作废待用租约 → 捕获 → 尺寸/字节预算校验 → 连接身份复核 → 存租约前复核原所有权与原代际，与既有 screenshot 语义逐字保持），输入动作在 backend 派发成功后于同一 `execute` 内调用它登记新租约；`Output` 显式新增 `input_dispatched`（派发事实，非效果证明）与 `observation_failure`（后置观测失败原因）——后置捕获失败不抹掉已发生的输入：调用仍成功返回、观测缺席并记录原因，同一调用内不自动重试；动作前观测的单次消费（一次消费尝试即烧毁租约）与 scope 校验不变，新观测是唯一可派发租约且同样单次。`examples/isolated_probe.rs` 输出补两字段。`crates/tools/src/computer.rs`：metadata 由旧的推导式 `input_dispatched`（jpeg/permissions 缺席推断）改为显式字段并新增 `observation_failure`，输入动作结果 = Text 元数据 + 动作后 JPEG（同一 ContentPart::Image 通路进持久消息与重放）；descriptor 与 observation_id schema 说明同步为新回合语义（输入结果自带新观测；观测失败时输入已派发、需再次截图）。工具消息、事件与 Provider 图片编码链路零改动。

native 路径裁决（范围自最小）：CU-06/07/08 交付的 native 动作 API（semantic_action / targeted_input / pointer_action / move_window）本轮未接入模型可见工具。理由：完整接线需要目标发现、授权台账、绑定与观测的宿主装配（CU-03/16 的生产化与 CU-17/18 的 GUI 目标选择 / 观测查询），远超本任务文档四条实施范围；且 CU-06/07/08 真实验收因控制台锁屏未执行，未验收的本机后台动作不应进入生产工具面；CU-11（图像能力前置门控）亦未落地，先暴露新截图工具会制造「截屏后才失败」的既知问题。拒绝语义（Pointer 全族终态拒绝、能力矩阵、授权 fail-closed）由各 CU 既有回归继续钉住，待接线时透传 Dispatched / Partial / UnknownEffect 终态。

实际命令与结果（2026-10-07，macOS 26.6.2 arm64 本机，仓库统一 kache wrapper）：`bash scripts/test.sh computer-use` 94 项全绿（CU-08 基线 92 + 新增 2：`input_returns_its_fresh_observation_in_the_same_call` 主路径、`capture_failure_after_dispatch_keeps_the_input_fact` 关键失败路径——后端调用计数钉住不自动重试）；`bash scripts/test.sh tools` 83 项全绿（新增 1：`input_action_returns_dispatch_fact_and_fresh_image_in_one_result`，含序列化往返）；`bash scripts/test.sh app` 275 + 2 + 1 项全绿（审批 / 持久化 / resume 不重执行等既有回归保持）；`cargo check -p pawork-computer-use --examples --offline` 通过。--host 真实子进程链路未跑：本轮不触碰 Host 子进程边界，工具结果持久化 / 重放由 app 既有回归覆盖。

复审修复（2026-10-07，6.1 sol 审查 2 必须修 + 1 建议修，全部落地）：① P1 后置捕获跨 release 签发租约——`capture_observation` 增派发代际锚点参数（输入路径传 `dispatch_generation`、截图路径传 `None`），锚点核对与 generation 推进合并到同一临界区：最后一个输入事件后发生 release、同 scope 排队调用重认领时，旧调用不再采用新代际签发租约，而是保留派发事实、按观测失败返回、不签租约不继续捕获；新增回归 `release_between_dispatch_and_observation_never_mints_a_lease`（Fake 的 Click 派发后 fire 点注入 abandon + 同 scope 重认领，钉住交错、输入事实与无租约残留）。② P2 Host 超时抹掉已派发事实——`ComputerTool` 的 `default_timeout_ms` 15s → 20s（统一动作与捕获预算：覆盖 RFB 输入 8s + 捕获 8s 串行）；scheduler 超时分支改为保留协作收口后工具交出的成功终态（输入已派发、观测因取消失败的成功结果沿既有结果链进入持久化），工具只回错误时仍按 Timeout 回执——与非超时取消路径的工具终态原样透传对齐；新增回归 `timeout_keeps_a_finalized_success_from_the_drained_tool`（scheduler 层）与 `dispatched_input_survives_observation_failure_through_the_scheduler`（真实 ComputerTool + 注入捕获失败的 backend 经 scheduler：input_dispatched=true、observation_failure 精确断言、无图片、后端计数钉住派发与单次捕获）。③ 建议修：人工验收的断线时机表述修正（见下段）。复审后重跑 `bash scripts/test.sh computer-use tools app`：95 / 85 / 275+2+1 全绿。

真实 / 人工结果：未执行——隔离桌面 Docker（127.0.0.1:5905）本机未运行，真实桌面上的动作+新观测闭环、观测失败终态与跨 Run 冲突待人工验收（先起 `crates/computer-use/desktop/compose.yaml`，再以 `examples/isolated_probe.rs` 或真实 Host 会话核对：输入后同一结果携带新截图与新 observation_id；确认输入已派发后、后置截图完成前断开 RFB，该输入应返回 input_dispatched=true + observation_failure 且输入已发生——单纯先断线再输入只能得到输入错误，不能证明派发事实保留）。

遗留缺口：真实隔离桌面人工验收待执行，通过前 ROADMAP 隔离路径保持「等待人工验收」、不标 ✅；native 动作的生产接线（含 Dispatched / Partial / UnknownEffect 终态透传与 CU-11 图像门控前置）待开发，按上文裁决另行任务推进；后置捕获目前不设额外稳定等待（RFB 帧在输入后即时读取，动画进行中的状态属于「此刻真实状态」，如需固定延迟属后续产品裁决）。
