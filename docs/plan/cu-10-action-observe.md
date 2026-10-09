# CU-10 · 动作与新观测的 Host 闭环

> 状态：✅ 完成（模拟验收，P3）；定向自动门禁与连接层 / CLI 控制循环模拟通过，独立 R3 复审 PASS。完成日期：2026-10-09。按用户本轮授权接续 R7，已补 #4 Engine 取消事实持久化与 #5 CLI 显式共享 Host 审批，并关闭初审和复审全部阻断问题；真实平台边界按下方记录排除，不宣称真人或发布验收。前置：[CU-04](cu-04-window-observation.md)、[CU-06](cu-06-semantic-actions.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

减少单独截图/动作消耗的回合，同时诚实区分输入派发和 UI 接受。

## 当前依据与读取范围

隔离路径已在同一输入结果携带新观测；native 模型工具与共享 Host 接线接续历史 R7 修复，本轮 Engine / CLI 缺口已实现，修后证据见本页末尾，独立审查尚未结束。输入成功只证明派发，不能证明 UI 接受。ToolExecutionCompleted / MessageCommitted 与图片沿用既有持久消息与重放通路；下方 R1～R7 是当时的源码、授权与验证记录。

[tools Spec](../spec/crates/tools.md)、[engine Spec](../spec/crates/engine.md)、[app Spec](../spec/crates/app.md)、[持久化流程](../spec/flows.md)。

2026-10-08 补 native 生产接线，基线 `e0c6f52f9853cf641f6968075aba2d6a824bbdb4`。沿用 CU-09/16 的 Host 目标与授权台账、CU-17/18 的 GUI 入口及 CU-11 前置图像门控，把模型、GUI 与父子 Core 接到同一共享状态。不重做隔离 RFB，不新建第二台账或持久事件旁路；网站身份缺信号和 pointer 三族 Unsupported 继续 fail-closed。送审前补正覆盖同一 Core 内投影 Run 的审批、锁外 AX 探测和截图持久化证据；修后交付与证据补正均 completed / error=null。随后独立审查返回 ISSUES（见下段），原离线通过数不证明这些新发现缺口已闭合；Mac 解锁不证明真实验收条件满足。

## 实施范围

2026-10-09 用户明确要求完成所有剩余任务（环境无法验证的部分除外），人工验收以模拟方式执行，执行模型改为 `zai/glm-5.3 max`、review 改为 `gpt-6.1-sol max`。本轮授权包含完成上述非破坏性 Engine / CLI 缺口，采用已审方案中的 CLI 显式 opt-in；下方历史记录里的「待授权 / 无人工回复」描述当时状态，不再作为本轮延期依据。继续保持现有架构、安全、wire/schema 和单一 Host 台账，模拟证据与真实平台缺口分开登记。

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

native 历史裁决（2026-10-07，当时未接线）：CU-06/07/08 交付的 native 动作 API（semantic_action / targeted_input / pointer_action / move_window）本轮未接入模型可见工具。理由：完整接线需要目标发现、授权台账、绑定与观测的宿主装配（CU-03/16 的生产化与 CU-17/18 的 GUI 目标选择 / 观测查询），远超本任务文档四条实施范围；且 CU-06/07/08 真实验收因控制台锁屏未执行，未验收的本机后台动作不应进入生产工具面；CU-11（图像能力前置门控）亦未落地，先暴露新截图工具会制造「截屏后才失败」的既知问题。拒绝语义（Pointer 全族终态拒绝、能力矩阵、授权 fail-closed）由各 CU 既有回归继续钉住，待接线时透传 Dispatched / Partial / UnknownEffect 终态。以上锁屏与 CU-11 未落地是当时条件，不作为 2026-10-08 继续延期的依据；本次以当前源码、平台矩阵和实际验收条件为准。

实际命令与结果（2026-10-07，macOS 26.6.2 arm64 本机，仓库统一 kache wrapper）：`bash scripts/test.sh computer-use` 94 项全绿（CU-08 基线 92 + 新增 2：`input_returns_its_fresh_observation_in_the_same_call` 主路径、`capture_failure_after_dispatch_keeps_the_input_fact` 关键失败路径——后端调用计数钉住不自动重试）；`bash scripts/test.sh tools` 83 项全绿（新增 1：`input_action_returns_dispatch_fact_and_fresh_image_in_one_result`，含序列化往返）；`bash scripts/test.sh app` 275 + 2 + 1 项全绿（审批 / 持久化 / resume 不重执行等既有回归保持）；`cargo check -p pawork-computer-use --examples --offline` 通过。--host 真实子进程链路未跑：本轮不触碰 Host 子进程边界，工具结果持久化 / 重放由 app 既有回归覆盖。

复审修复（2026-10-07，6.1 sol 审查 2 必须修 + 1 建议修，全部落地）：① P1 后置捕获跨 release 签发租约——`capture_observation` 增派发代际锚点参数（输入路径传 `dispatch_generation`、截图路径传 `None`），锚点核对与 generation 推进合并到同一临界区：最后一个输入事件后发生 release、同 scope 排队调用重认领时，旧调用不再采用新代际签发租约，而是保留派发事实、按观测失败返回、不签租约不继续捕获；新增回归 `release_between_dispatch_and_observation_never_mints_a_lease`（Fake 的 Click 派发后 fire 点注入 abandon + 同 scope 重认领，钉住交错、输入事实与无租约残留）。② P2 Host 超时抹掉已派发事实——`ComputerTool` 的 `default_timeout_ms` 15s → 20s（统一动作与捕获预算：覆盖 RFB 输入 8s + 捕获 8s 串行）；scheduler 超时分支改为保留协作收口后工具交出的成功终态（输入已派发、观测因取消失败的成功结果沿既有结果链进入持久化），工具只回错误时仍按 Timeout 回执——与非超时取消路径的工具终态原样透传对齐；新增回归 `timeout_keeps_a_finalized_success_from_the_drained_tool`（scheduler 层）与 `dispatched_input_survives_observation_failure_through_the_scheduler`（真实 ComputerTool + 注入捕获失败的 backend 经 scheduler：input_dispatched=true、observation_failure 精确断言、无图片、后端计数钉住派发与单次捕获）。③ 建议修：人工验收的断线时机表述修正（见下段）。复审后重跑 `bash scripts/test.sh computer-use tools app`：95 / 85 / 275+2+1 全绿。

真实 / 人工结果：未执行——隔离桌面 Docker（127.0.0.1:5905）本机未运行，真实桌面上的动作+新观测闭环、观测失败终态与跨 Run 冲突待人工验收（先起 `crates/computer-use/desktop/compose.yaml`，再以 `examples/isolated_probe.rs` 或真实 Host 会话核对：输入后同一结果携带新截图与新 observation_id；确认输入已派发后、后置截图完成前断开 RFB，该输入应返回 input_dispatched=true + observation_failure 且输入已发生——单纯先断线再输入只能得到输入错误，不能证明派发事实保留）。

遗留缺口：真实隔离桌面人工验收待执行，通过前 ROADMAP 隔离路径保持「等待人工验收」、不标 ✅；native 生产接线的当前交付与待验范围见下段，不再以历史未接线裁决延期。隔离后置捕获目前不设额外稳定等待（RFB 帧在输入后即时读取，动画进行中的状态属于「此刻真实状态」，如需固定延迟属后续产品裁决）。

### Native 生产接线历史交付与独立审查（2026-10-08 至 R7，未包含本轮新实现）

实现线程 `01a11aef-d5f5-77f1-a480-bf2fc2bcee83` 使用 `xai/grok-4.7` / xhigh；修后 turn `01a11b45-9e7a-7c20-9c31-ca729652b96b` 与仅补证据 turn `01a11baa-e673-7881-84fa-f787b7e01c15` 已核 completed / error=null。13 个 Rust 文件仅在 app、tools、computer-use：AppCore 持一个 `Arc<NativeHost<HostSurface>>`，GUI、工具表重建与 child 共用 registry / backend / 窗口代际，child 不继承 parent scope grant。`computer_native` 仅接受 bundle 身份或 Host 句柄，request 只登记待批目标；GUI 列表 / 审批结合本 session 的持久 Run 投影与活跃 native scope。同 Core 内回归覆盖 CLI / child 投影及跨 session / closed / terminal 拒绝，但普通独立 CLI 进程的目标审批未接到执行进程台账。RunService 的 Drop 守卫覆盖 open 后早退，终态关闭原 scope，不因 GUI 断连、切任务或关窗取消 Run；实际 Core.native 完整生命周期证据仍有审查缺口。

NativeHost 用锁外 ProbeSnapshot 承接长 AX 读取，登记锁内复核并消费单次凭证；平台动作不克隆 authorizer 或续授权。Host 一次动作后新观测保留 Dispatched / Partial / UnknownEffect 与 observation_failure，不自动重试；Once 已消费时不额外授权后置截图。正常返回结果沿 AgentTool → Scheduler → Engine 的 ToolExecutionCompleted / MessageCommitted 与 ImageSource::Base64 持久化，既有观测查询 / resume 只读历史；在途撤销 / 接管复核、公开平台接缝和取消落库尚未满足完整闭环（见独立审查）。CU-11 暴露及臆造调用双门控、Policy / untrusted / ReadOnly / 自动审批拒绝维持前置；网站和 pointer click / drag / scroll 三族继续 fail-closed，无全局或前台输入回退。macOS 活动信号仅在入口只读 frontmost bundle，不记录全局键盘内容；它不是在途让出、连续用户输入或真实接管及时性的验收证明。

修后实际验证（日志目录 `/tmp/pawork-cu10-native-host-wiring`，串行 Cargo）：`cargo test -p pawork-computer-use --offline --lib` 99 passed；`cargo test -p pawork-tools --offline --lib computer_native` 3 passed / 85 filtered（动作新图、后置失败不重试、长 tree_revision 期间撤销）；`cargo test -p pawork-app --offline --lib --features ui-fixture -- computer_targets_follow_projection_runs_without_gui_registry computer_native_engine same_provider_child_reuses_adapter vision_gate_withholds_computer` 5 passed / 299 filtered；`cargo check -p pawork --offline` exit 0；13 个任务 Rust 文件 `rustfmt --edition 2021 --config skip_children=true --check` exit 0。App Engine 两项验证真实 NativeHost / Tool / Scheduler / Engine 与临时 SessionStore 的截图持久化、既有 GUI 历史回读及拒批零捕获；它们通过替换工具注入 fake Host，不证明实际 Core.native 的终态释放、完整 GUI 目标审批或动作派发后的持久化。另有 CLI / child 投影审批及父子共享、grant 不继承的定向回归。

证据补正保留了原 Cargo filter 语法 exit 1、四条辅助搜索 exit 1、格式检查 exit 1；`05-rustfmt-check-before.log` 只点名两个新文件且没有原命令行，不称全仓检查。格式命令实际触及 gui_host/mod.rs，最终相对 HEAD 无 diff；`03-app.log` 的 NativeEngineRun.provider 未读警告属于新增测试字段，MCP with_auth_token 为基线警告，`04-check.log` 另有 App 库 dead_code 与 block future-incompat 提示。中间编译失败曾发生、后续已恢复，保留日志中没有其原文，不重建或伪造。实现线程曾为补输出读取自己的 Codex 全局会话文件，已如实登记越界事实，补证据 turn 未再读取；这不计入合法产品验证。原 cmp.log 仅比较复制件；补证据的 independent.diff 与指挥独立重建均和原 final.diff 逐字节相同：238290 字节、SHA256 `7f28ee57194268348b42532d4a6e793f015418d7850355a381c6c5ebeda5465d`（含实现源码与当时两份任务文档）；指挥随后同步三包 Spec 和本记录，完整受审差异另行生成。

独立审查：唯一 reviewer `01a11bba-e2e4-76a1-85b8-47049b6022b8`（`gpt-6.1-sol` / max），turn `01a11bba-e729-7a31-9cb5-931ad61cacd8` completed / error=null，结论 ISSUES：P1 在途撤销失去原 permit 关联、P1 长 AX 后不再复核真实 target activity、P1 工具丢弃应用发现 detail 且 read_tree 未回节点 / 截断、P1 Engine 取消时丢弃已收口派发结果、P2 普通独立 CLI 的目标审批不可达、P2 公共 detached API / HostSurface 绕过授权与单次凭证；非阻断 P3 为实际 Core.native 完整生命周期缺证据。受审 root-review.diff 350069 字节、18 文件、SHA256 `55b04967d7d78b3fcd35a55e322b7eb2c5d6b83f66c2c64093a3607e2d7feb03`，指挥与 reviewer 独立重建一致；202 相对链接 missing 0、diff 检查通过，不替代行为审查。ISSUES 原文已回同一 Grok xhigh 实现线程，修复 turn `01a11bd0-6706-7182-b6b5-2b3a44f3de41` 已核 active / inProgress / error=null；不重复派发。范围内先修，涉及默认只读 Engine / CLI 路由的必要扩写先交具体方案，不自行落地；修后仍由同一 reviewer 复审。

首轮审查修复交付：上述 turn `01a11bd0-6706-7182-b6b5-2b3a44f3de41` 现已核 completed / error=null。新增 approval.rs 后共有 14 个 Rust 文件，仍限 app、tools、computer-use；提交了原台账代次与在途 dispatch ticket、探测后 / 派发前 / 事件组间活动复核、模型 Text 中应用发现与 AX 节点 / 截断输出、私有 HostSurface 平台字段的修复，尚未复审。AppCore.native 改为共享 `Arc<dyn NativeControl>`，既有 Engine 回归改用同一 Arc 经 Core 授权 / 绑定 / 执行，并核对成功与拒批终态 scope / grant 释放；这些是平台替身回归，不代表真实 native 验收。Engine 取消落库与普通独立 CLI 审批没有实现，仅交 `/tmp` 方案；原完成条件不缩减，不把方案当作闭合。

该交付实际命令：computer-use `--lib` 99 passed；tools `--lib computer_native -- --test-threads=8` 4 passed / 85 filtered；App 原四 filters 5 passed / 299 filtered；正式 `cargo check -p pawork --offline` exit 0。指挥从当前 Git 独立重建完整 tracked + 两个 untracked 差异，与 review-r1-rebuilt.diff cmp exit 0：385705 字节、19 文件、SHA256 `7877f35b20e2e235423ee1a3e9ace986ea4d46f55123be0cba7272ed0b5686bf`，另存 root-r1-delivery.diff 不覆盖原受审快照。初次 Cargo E0449 与 tools 新回归 panic 均曾 exit 101，后续恢复；指挥通过合法 read_thread 取回两段真实失败尾输出并存 ROOT-R1-DELIVERY-NOTES.txt，原路径已被成功重跑覆盖，不假称当前日志保留失败全文。三次 CodeGraph 非法选项与四条无匹配查询 exit 1 同样登记，原输出无法完整恢复的部分不伪造。新增 macOS wrapper dead_code 与 NativeEngineRun.provider 警告分别记录，不统称基线。

送复审前兼容性补正：指挥发现八个既有受授权保护的 macOS public wrapper 被收为 crate 私有，而未改的 macos_target_probe example 仍调用；上述 `--lib` / 正式 Host check 不编译该验收资产，不能证明其可用。已回同一 Grok xhigh 线程，turn `01a11c18-abb7-7212-b9c9-a9e9fddc77ea` 已核 active / inProgress / error=null，要求保留私有 detached / HostSurface 接缝、恢复受保护验收入口的最小兼容性并做该 example 定向类型检查；不执行真实探针、不启动第二个 Cargo。独立 reviewer 仍为原 Sol max，兼容性与证据补正稳定后才复审；涉及默认只读 Engine / CLI 的方案尚未获扩写授权。

兼容性补正交付：turn `01a11c18-abb7-7212-b9c9-a9e9fddc77ea` 已核 completed / error=null，只恢复八个原有授权包装的 public 可见性；detached 方法与 HostSurface.local 仍为 crate 私有，HostSurface 平台字段私有。`cargo check -p pawork-computer-use --example macos_target_probe --offline` exit 0，仅类型检查、未执行探针；macos.rs 定向 rustfmt exit 0，未重跑行为门禁。实现者未另存这两条原始输出日志，指挥通过合法 read_thread 回读完整输出并摘存 ROOT-R1B-DELIVERY-NOTES.txt，不冒充重新运行。指挥独立重建与 review-r1b-rebuilt.diff cmp exit 0：385280 字节、19 文件、SHA256 `5ece01230d4c6d21c5de1ea640237f6666c8bc86acc800dbc20ec5894c4e3e0b`，旧受审与交付快照均保留。

新增排版补正：指挥完整 14 文件 rustfmt 实际 exit 1，只有新增写入的 approval.rs 报差异；对照 HEAD 发现大部分为未改基线，仅回同一 Grok xhigh 线程修新增 retain / armed / fresh 三处。turn `01a11c26-4821-7aa3-887b-5ca3d27d247f` 已核 completed / error=null，仅该三处排版、无行为或断言变化、无 Cargo。实现者逐文件原始日志 review-r1c-fmt-* 与 exits 汇总记录 13 文件 exit 0、approval.rs 完整检查仍 exit 1（19 处未改基线差异）；HEAD stdin 对照同样输出 19 处 Diff，但命令返回 exit 0，不能称基线格式通过。指挥核对 r1b → r1c 差异仅这三处及 diff 索引 / 行号，并从 Git 独立重建，与 review-r1c-rebuilt.diff cmp exit 0：385212 字节、19 文件、SHA256 `d726e08b52b53277923a8d3fd5faf62d200bc256e618da19e100962b28a4e003`。三包 Spec 已按当前共享 NativeControl Arc、原台账 armed ticket / 代次、Text 节点 / 截断及公开授权包装边界同步；初审与各次交付证据不覆盖，新完整受审快照另行生成。

待裁决范围：Engine 取消落库（review-r1-issue-4-cancel-persist.md）与独立 CLI 目标审批（补充 review-r1b-issue-5-cli-grant.md）均只交 `/tmp` 方案，Engine / CLI / Protocol 等默认只读，尚无实现授权。取消方案复用既有 ToolExecutionCompleted / MessageCommitted 后再 RunCancelled、不重试；CLI 方案以执行 Core 的 requested TargetState 和真实 Scope 签发，不从 preview 猜身份、不自批、不把 grant 写库。补充方案指出 AppCore 的 target_states / grant_target 仍为 crate 私有，也尚未给出实际终端询问时机 / 接线的完整可执行裁决；另进程 GUI 必须破坏冻结协议的旧断言未证成，保持未决。两项方案将由原 Sol max reviewer 一并只读核对，不把方案或部分修复判为 PASS。

第二轮独立复审（2026-10-09）：沿用唯一 Sol max reviewer，turn `01a11c36-22eb-71a1-932c-7681dbf754a0` completed / error=null，结论仍为 ISSUES。原 #3 模型 Text 中发现 detail / 树节点 / 截断与 #6 公开平台接缝已关闭；#1 / #2 虽补 ticket 代次和活动复核，但事件组仍按「检查 → 耗时 AX recheck → 派发」执行，复核结束至实际投递前缺最后一次检查，范围内待补修。#4 Engine 取消丢弃结果与 #5 独立 CLI 的 GUI 目标审批均未实现；取消方案须保留成功或严格布尔 input_dispatched=true 的原结果，并明确回填同批取消 / 缺失项再走既有事件链；CLI 方案须让既有 GUI 入口访问执行 Run 的同一 Core，尚缺端点接入与 request 后、bind / action 前的等待 / 拒绝 / 取消路由，终端目标批准不能替代 GUI 明确批准。#7 同一 Core.native 生命周期部分补齐，但现有 Engine 调用仍只有截图，GUI 批准与动作后落库完整链仍缺非阻断证据。R2 完整快照 413179 字节、19 文件、SHA256 `7f7ad19753a4474b70603df8852c4ad2cbc84887e60876fc918f774d996d3965`，指挥与 reviewer 独立重建逐字节一致；源码快照 259446 字节、14 文件、SHA256 `37afe4ab8d3c4bb6ba69243f3cf514c496c8913aee3c4d46b74ec5d7b9db4609`，与 r1c 源码一致。送审前五份文档 206 相对链接 missing 0、diff 检查通过；reviewer 只读未编译 / 测试 / 写入。R1 的 99 / 4 / 5 与 r1b example 类型检查、r1c 格式结果只证明各自标注版本，不证明新发现缺口闭合；原受审快照不覆盖，ISSUES 原文保存于 `/tmp/pawork-cu10-native-host-wiring/review-r2-ISSUES.txt`。

R2 ISSUES 修后交付（2026-10-09）：同一 Grok xhigh turn `01a11c4a-b078-7363-89b7-95d1b2445328` 已核 completed / error=null，只改 macos.rs。`post_targeted_plan` 的 TextChunk / KeyTap / ModsDown 后续组（index > 0）在 AX recheck 与事件构造后、投递前再检查停止信号；ModsUp 保留本次已按下状态的释放。零投递时 Err(Cancelled)，已有投递时释放 held state 并返回 Partial，Host 的 DispatchStop 映射不变。新检查明确跳过 index 0，入口检查仍在计划 / event source / 首组事件构造前，因此 #1 / #2 不先行判 closed，首组边界交原 reviewer 复审。

该版本 `cargo test -p pawork-computer-use --offline --lib targeted_` exit 0：4 passed / 95 filtered；沿用既有计数测试，扩四种 recheck 内停止情况，分别保留 2/6、2/4、2/4 与零投递事实。macos.rs 定向 rustfmt 与 git diff --check 均 exit 0，未重跑 App / Host 或真实验收。实现者把 Cargo 输出摘录到 review-r2-NOTES.txt、未建单独测试日志；指挥一次猜测不存在的 review-r2-targeted-tests.log 读取失败，随后从合法 read_thread 获取未截断原始输出并存 ROOT-R2-DELIVERY-NOTES.txt，不冒充重跑或历史日志恢复。指挥独立重建与 review-r2-rebuilt.diff cmp 0：422292 字节、19 文件、SHA256 `9db520c9851188dd20d581599f2b89590c07ae995bc546dc7996829176b4212c`；除 macos.rs 外其余 13 个源码差异与 R2 受审版逐字节不变。指挥随后仅同步文档，新的完整受审快照另存。

#4 / #5 的新材料 review-r2-issue-4-proposal.txt 与 review-r2-issue-5-proposal.txt 仍只是方案：前者明确成功或严格布尔派发事实、同批取消缺项回填后沿既有事件链收口；后者建议 TTY run 路由到已服务的 GUI Host 同一 Core，并在 bind / launch 前锁外等待真实目标批准。Engine / CLI 未改、扩写未授权；首方案的 Diagnostic 处理和后方案的普通 CLI 行为、chat、参数与端点细节仍待只读审查，不能用方案关闭 #4 / #5。

第三轮独立复审（2026-10-09）：原 Sol max reviewer turn `01a11c63-2761-7fe0-89c2-597dcc39e01c` completed / error=null，仍为 ISSUES；#3 / #6 保持 closed，#1 / #2 后续组修复获认可，但 `index == 0` 豁免留下首组构造后、post 前的撤销 / 接管缺口，单组文本和无修饰 KeyTap 可能直接 Dispatched。四个新增情形确实覆盖后续组，未覆盖生产首组；既有恒 true cancelled 断言仍要求先投首组，应在同一计数测试内调整并覆盖首组，不新增框架。#4 未实现，最窄 Engine 方案须明确取消 Text content、保留原 sandbox.fallback Diagnostic，成功或严格布尔派发事实触发整个 invocations 配对、Denied 保持 Authorization、其余 Cancelled、不再次请求 Provider。#5 未实现，自动路由所有 TTY run 会改宿主 / workspace / 配置与通用审批位置，reviewer 建议显式 opt-in，但未获用户裁决；run/chat 参数、端点 / token 认证、附件、订阅 / ACK / heartbeat / 终态、Ctrl-C 与断连语义均须先写清。bind 内等待受 native 25 秒工具 timeout 限制，应核 App scheduler 前等待且只对明确支持 GUI 等待的 Run 生效，不能让 headless / 非 TTY 陷入同样等待。#7 保持非阻断，仍缺 GUI 明确批准 → native action → Engine 落库 → 查询 / 零重执行证据。

R3 完整受审 root-r3-review.diff 为 424944 字节、19 文件、SHA256 `7d116363ed6bb19670755f1e6719743b721168505c9723d39a45086dbf43a6e5`；源码 265899 字节、14 文件、SHA256 `dd5df2e80e587aeec8df8065030ab10daac5a313ed1b7253c21afe11e564115e`。指挥双重重建与 reviewer 两种内存重建 cmp 0；207 相对链接 missing 0、diff 0、索引空。reviewer 只读，无 Cargo / 写入；三条猜测源码路径的 rg exit 2 通过合法 read_thread 取回原始输出并登记 ROOT-R3-POST-REVIEW-NOTES.txt，前两项定位已恢复、第三项不影响裁决，不作产品失败或历史恢复。ISSUES 原文保留于 `/tmp/pawork-cu10-native-host-wiring/review-r3-ISSUES.txt`，首组范围内补修仍回同一 Grok xhigh 线程；Engine / CLI 只继续准备方案，不自行扩写。

R3 修后交付（2026-10-09）：原 Grok xhigh turn `01a11c73-306f-7783-8cfd-67aeac22a41d` completed / error=null，只改 macos.rs，去掉 `cancelled_before_post` 的首组豁免；TextChunk / KeyTap / ModsDown 包括首组在内均在构造后、post 前检查 Host 停止状态。零派发 Err(Cancelled)，已有派发释放本次 held state 后 Partial，ModsUp 与 DispatchStop 映射保留。沿用原计数测试调整生产组合键 / 25 字文本的旧恒取消断言，补单字符文本 / 无修饰键两种单组计划，均零投递；后续组复核停止与创建失败情形保留。格式化后 `cargo test -p pawork-computer-use --offline --lib targeted_` 4 passed / 95 filtered，立即保存的 CARGO_EXIT:0；macos.rs rustfmt 与 diff 检查 exit 0。格式首次 exit 1，成功重跑覆盖旧日志；两次格式化前测试输出通过、原路径亦覆盖。指挥通过合法 read_thread 摘存真实命令 / 状态，不伪造失败原文，详见 ROOT-R3-DELIVERY-NOTES.txt。当前完整交付 430422 字节、19 文件、SHA256 `a3874213d7844edf4215ee4bde0fded84ea30cce984a0afde4e556ac7f1a58ea`，指挥独立重建 cmp 0，其余 13 个源码差异与 R3 受审版逐字节不变；新的受审快照另存，尚不将 #1 / #2 判 closed。

新方案 review-r3-issue-4-proposal.txt 保留取消 Text / Denied / 原 Diagnostic 与既有事件链；review-r3-issue-5-proposal.txt 推荐显式 opt-in run/chat 客户端，普通 CLI 不自动路由，Provider/model 与 trust/approval 覆盖不能静默写共享 Host；远程附件与 branch 等无现成安全通路时明确拒绝，原认证 / workspace / 订阅 / 终态与 scheduler 前锁外等待须独立审查。两项均未实施。指挥已单独请求 Engine 两源码文件和 Engine Spec 的最小扩写，尚未收到用户回复，预选项不算授权；CLI 产品选择与扩写仍未裁决，不自行改变默认行为或冻结 wire。

第四轮独立复审（2026-10-09）：原 Sol max reviewer turn `01a11c84-9872-77a3-b4d5-c55dfe73279e` completed / error=null，结论 ISSUES。#1 / #2 已关闭：首组豁免删除，TextChunk / KeyTap / ModsDown 在构造后、实际 post 前检查原 Host 停止状态；生产组合键、多块文本、单块文本和无修饰 KeyTap 恒取消均 Err、零投递，原后续组 / 释放 / 创建失败断言保留。#3 / #6 保持 closed，不重改其源码；这不是连续用户输入或真实接管验收。

#4 未实施。reviewer 认可两份 Engine 源文件加 Spec 的最窄方案可交用户授权；同一混合批次测试须分别证明 success=true 无派发标记，以及 success=false 且严格布尔 input_dispatched=true（保留 Diagnostic），并覆盖 Denied、缺项、非事实字符串；原无事实取消的零 Completed / tool MessageCommitted 与一次 Provider 请求锚点保留。Engine 扩写此前已单独询问，尚未收到用户回复，不重复询问或自行实现。

#5 未实施，方案仍待补：GUI server 将 socket 来源盖章为 LocalGui，R3 的 RemoteGui 判断已由 reviewer 明确纠正；不能用 RemoteGui 标记等待或断言图片被系统拒绝。候选沿用同一 ConnectionManager，App 按服务端 client_id 查握手名，在 RunStart 固定本 Run 等待 bool，经内部 AppCore / RunService / SessionLoopCtx 到 scheduler 前、Policy / 通用审批后。握手名只表示允许等待，不授身份或更高权限；NativeControl / NativeHost 还需原 registry 真实 requested pair 的锁外等待与 grant / revoke / close 唤醒、周期取消检查，request 被撤销后结束，25 秒工具工作 timeout 不含等批。没有证据要求改 wire / schema 或 GuiServer / Protocol / Storage / Client / Desktop。显式 opt-in、首版参数限制、socket / token、cwd 与 resume workspace 核对、API 1.27 下限及事件丢失后的 RunStatus 收口仍属待裁决方案。R4 原文保存于 `/tmp/pawork-cu10-native-host-wiring/review-r4-ISSUES.txt`，已回原 Grok xhigh 线程仅补 `/tmp` 可审阅方案，不扩写仓库。

R4 完整受审 root-r4-review.diff：433099 字节、19 文件、SHA256 `4af6b30edb42c49876bb878b23ce784b905e0f9f26858fddfb74c9ff33c9077d`；源码 root-r4-source.diff：268871 字节、14 文件、SHA256 `ecd5e739beaa79242c95579860f9d548201963585268d9b9551016ad6873d652`。reviewer 两种独立内存重建及最终复核 cmp 0、源码与交付 cmp 0，209 相对链接 missing 0、diff 0、索引空。root / reviewer 无 Cargo；一条猜测 handler.rs 的 rg exit 2 已在 gui-server/lib.rs 恢复定位，合法 read_thread 原始失败输出存 ROOT-R4-POST-REVIEW-NOTES.txt，不作产品失败。首次读回直接展示全部 items 导致展示截断，随后从同一已存结构化结果仅取最终消息及真实非零命令，不重新运行或伪造历史。

第五轮方案复核（2026-10-09）：同一 Sol max reviewer turn `01a11ca7-f565-71f3-b5b8-74460359c0f4` completed / error=null，仍为 ISSUES，#1 / #2 / #3 / #6 继承 closed，#4 / #5 未实现、#7 非阻断 OPEN。#4 最窄方案可执行，仅统一事件顺序文字为逐条 Completed 紧随该条可选 sandbox.fallback Diagnostic，随后唯一 MessageCommitted、最后调用者 RunCancelled。#5 首版须拒绝 `/compact`（GUI registry 禁用且无 handler）；RunStatus 只返 running / unknown，不能补回终态，纠正 R4 未实施建议：沿原实例重连、订阅与 Resume，必要时用 SessionGet 分页 Timeline 查原 run_id 的 RunCompleted / RunCancelled / RunFailed；unknown 和分页 complete 均不是终态，不可达明确报告终态未知、不重建或自动取消 Run。同锁谓词循环与 Condvar 已能避免丢唤醒，普通通知 generation 未证必要；原授权代次与单次凭证不放宽。候选仍为十个生产源文件与紧相关既有回归、三包 Spec，不要求改 wire / schema / 依赖。ISSUES 原文已回同一 Grok xhigh 线程，仅另存 `/tmp` 新方案、保留旧产物；Engine 与 CLI 扩写均已各询问一次、无人类回复，reviewer 对 CLI「尚未询问」的编排描述由 root 更正，不重复询问或自行实施。

R5 受审完整差异 437143 字节、19 文件、SHA256 `57dcc34aac8fd527f651ccfe2ddb047f35b8d5295410fb771686f3a34d34c6da`；源码 268871 字节、14 文件、SHA256 `ecd5e739beaa79242c95579860f9d548201963585268d9b9551016ad6873d652`，与 R4 cmp 0。reviewer 独立重建及 root 裁决后、文档回写前复核均 cmp 0，211 相对链接 missing 0、diff 0、索引空。两条辅助定位分别 exit 2 / 1，已从合法 read_thread 取回原始输出存 ROOT-R5-POST-REVIEW-NOTES.txt，实际文件定位已恢复，不计产品失败；无 Cargo 或真实验收。原文保存于 `/tmp/pawork-cu10-native-host-wiring/review-r5-ISSUES.txt`，旧快照不覆盖。

R5 方案补正交付（2026-10-09）：同一 Grok xhigh turn `01a11cb4-fc51-7790-97d5-25888313be69` completed / error=null，只另存两份 `/tmp` 方案与笔记；#4 新方案 7528 字节、SHA256 `0913e1fc9d1f4f38ea85f2b50d7d12cfe334df97d6ded57454cee054a1477c33`，明确逐条 Completed / 可选 Diagnostic 交错。#5 新方案补正 `/compact`、重连 / Timeline 和 Condvar，随后 root 发现 live RunChanged 状态与历史 Timeline kind 名称混用；原线程 turn `01a11cbf-1c50-7e11-a43f-ec291f8acce2` completed / error=null，仅另存 R5b #5：14821 字节、SHA256 `50d8625778883baacfba3680ceaa1922ac401d3b980272aeb34fa800b1c39158`，live 匹配 RunState::{Completed, Cancelled, Failed}，历史匹配原 run_id 与 TimelineItemKind::{RunCompleted, RunCancelled, RunFailed}；其它未证明状态沿未知路径、不臆造终态。R5b 与 R5 仅标题及两句说明差异，候选写入集不扩；R4 / R5 旧提案与快照不覆盖，源码未变、无 Cargo，新方案待原 reviewer 复核，不关闭 #4 / #5。原始命令与输出分别保存在 ROOT-R5-PROPOSAL-DELIVERY-NOTES.txt 和 review-r5b-*.log；前者 CodeGraph 片段已在合法 read_thread 截断、实现者以 Python 读取简单范围的约定偏离如实登记，后者一条错误文件内枚举查询 exit 1 后恢复定位，均不作为产品门禁。

第六轮只读方案复核（2026-10-09）：原 Sol max reviewer turn `01a11cc4-1184-7cd3-bd6a-1b5035daca27` completed / error=null，结论仍 ISSUES，但 #4 / #5 方案文字已闭合、均可交用户裁决，无新增扩写要求；不构成实现 PASS 或人类授权。#1 / #2 / #3 / #6 继承 closed，#4 / #5 未实施、#7 非阻断 OPEN。R6 完整受审 441681 字节、19 文件、SHA256 `1edf5b7e0edd782a922030f93133feacbb0d3209673378df3a156410fe126abc`；源码 268871 字节、14 文件、SHA256 `ecd5e739beaa79242c95579860f9d548201963585268d9b9551016ad6873d652`，与 R5 / R4 cmp 0。reviewer 双重独立重建和 root 裁决后文档回写前复核均 cmp 0；211 相对链接 missing 0、diff 0、索引空，无 Cargo / 写入 / 新验收。原文保存在 `/tmp/pawork-cu10-native-host-wiring/review-r6-ISSUES.txt`。Engine 与 CLI 均已各询问一次且无人类回复，不重复询问，不提交部分修复绕过整体 ISSUES。

R6 后的范围内准备（补证前）：root 经 CodeGraph 复核当时 App 主回归，确认是直接 Core grant / bind 后只有模型截图，不能证明 GUI 明确批准到动作的链路。原 Grok xhigh 线程随后只升级 services/run.rs 中既有两个 native Engine 回归及必要最小 fixture，不扩 Engine / CLI 或冻结契约。

离线生命周期补证交付（2026-10-09）：原实现 turn `01a11cce-6000-76b0-8c76-3dab65e08efd` completed / error=null，仅改上述测试与 fixture。真实 Run 自行 open scope 并持久化 RunStarted，模型先 request，在 bind 回合以 Notify 暂停；`GuiHostAdapter::from_locked` 共用同一 Core，通过 LocalGui `ComputerTargetApprove` 明确批准 ForRun 后放行。模型 bind 从真实工具消息提取 Host 窗口句柄，继而 move_window，经 NativeComputerTool → Scheduler → Engine 一次 move、一次新捕获，ToolExecutionCompleted / 工具 MessageCommitted 携 ImageSource::Base64 JPEG；resume 及既有 ObservationList / ObservationImageRead 回读后计数仍为 1、终态 scope / grant 释放。关键失败项同样已获 GUI 目标批准，但通用工具审批拒绝 move_window，request / bind 成功、动作无 ExecutionStarted、零 move / capture、无持久图片且终态释放；不是 GUI 拒绝目标授权的测试。Provider 与平台仅替代外部边界，不作为真实界面 / AX 验收，也未修 #4 / #5。

实际命令 `cargo test -p pawork-app --offline --lib --features ui-fixture computer_native_engine` 首次 exit 101：ReleaseOnDrop 错将 Notify 当 Arc；修为持有 Arc<ApprovalGate> 后同命令 exit 0，2 passed / 302 filtered、0.09s。两次 Cargo 串行，MCP with_auth_token 与 block future-incompat 既有警告保留。实现者未另存原始 Cargo 日志，root 从合法 read_thread 获取两份未截断输出存 ROOT-R6-LIFECYCLE-DELIVERY-NOTES.txt，不称重跑或历史恢复；两条辅助定位 exit 1 无返回输出，按状态登记、不补造。补证前交付 465153 字节、SHA256 `b2fc0c8e1b714ecbf92e25a74448562193694ebddcfd1a22e2d6d4ff1e1a431c`，root 独立重建 cmp 0；其它 13 源码与 R6 一致。

送审前格式补正：root 定向 run.rs rustfmt exit 1（十处新测试排版，原输出 root-r7-lifecycle-rustfmt-before.log）；原 Grok xhigh turn `01a11d17-cf99-7d21-9187-61fd33ae3514` completed / error=null，只补排版、无 Cargo。有效 rustfmt / diff 检查 exit 0，首次 wrapper 因 zsh 的 status 只读而 exit 1，review-r6-lifecycle-format-rustfmt-check.log 无有效退出码、不用于通过结论。手写 notes 实际以 shell cat 创建、未遵守 apply_patch 约定，root 新笔记如实登记、不伪称原方式合规。格式交付 review-r6-lifecycle-format-final.diff 为 465234 字节、19 文件、SHA256 `31a6a4a35771cac88b6fcea031fc0413eb87ecdd29f43d53b0f5f9066bd8a40e`；root 独立重建 cmp 0、其它 13 源码 cmp 0、生产前缀与 R6 cmp 0，stdin rustfmt 预期与当前 run.rs cmp 0。源码快照 290588 字节、14 文件、SHA256 `2633ca232a4d443b7002653ab437faa2226e80c6810618aacf566a8ff99c51ad`。root 不跑 Cargo，随后仅同步 App 测试资产、plan 与 ROADMAP；旧快照保留，新的完整受审快照另存，离线补证尚未获复审结论。

第七轮独立复审（2026-10-09）：原 Sol max reviewer turn `01a11d26-c703-7d33-bc50-e72e62a8cdd5` completed / error=null，结论仍 ISSUES、无新增阻断。#7 离线生命周期补证充分、获认可：同一 Core 的真实 GUI handler 批准、模型 Host 句柄、一次动作 / 新捕获、persist-before-render、数据库历史同图、查询 / resume 零重执行及终态释放均有产品链断言；已批准目标后的通用工具拒批保持零动作 / 捕获。它不覆盖真实 Desktop / socket / GUI 点击、AX / TCC / Provider / 并发接管，因此 #7 保持非阻断 OPEN。#1 / #2 / #3 / #6 继承 closed，#4 / #5 未实施、awaiting_user，均已各询问一次、无人工回复；不把补证认可当作整体 PASS。

R7 受审完整快照 root-r7-review.diff：468519 字节、19 文件、SHA256 `41c6104b65365b719f714d6dd2fdae8d0167e916b4355ac7c3808b7685e659e7`；源码 root-r7-source.diff：290588 字节、14 文件、SHA256 `2633ca232a4d443b7002653ab437faa2226e80c6810618aacf566a8ff99c51ad`。reviewer 两种独立内存重建与送审快照 cmp 0；生产前缀、其余 13 源码与 R6 cmp 0，211 相对链接 missing 0、diff 0、索引空，root 裁决后文档回写前再次 cmp 0。原文另存 review-r7-ISSUES.txt。两条诊断断言 exit 1 分别误要求删空白后完全相等（rustfmt 可调整尾逗号）及误认为相对旧 R6 快照只有三份文档变化（另两份 Spec 已含 R6 裁决元数据）；实际差异核对后恢复，不作产品失败。合法 read_thread 的原始非零输出与恢复命令保存在 ROOT-R7-POST-REVIEW-NOTES.txt，截断读取保持截断标记、不假称全文恢复。reviewer 只读、无 Cargo / 写入，root 仅同步已验证审查元数据，不提交部分修复绕过整体 ISSUES。

R7 当时的真实验收与剩余边界：未跑 Desktop / socket / 真实 AX、TCC、Provider、用户并发输入 / 接管、真实模型链动作后持久化端到端或隔离 Docker 人工场景。真实模型链固定当次 `--provider opencode-go --model glm-5.3-flash`，disabled_models 正确拒绝不得绕过或换模型。当时 #4 / #5 未实现、扩写未获人工回复，#7 离线补证获认可、真实验收仍缺。该授权状态已被 2026-10-09 用户新指令覆盖；它不是本轮延期依据，也不作为修后通过证据。

### 本轮 GLM max 实现与模拟验证（2026-10-09，独立初审 ISSUES）

执行对话 `01a11ea4-c42d-7080-814e-ad8205ce3f9f`，`zai/glm-5.3 max`；turn `01a11ea4-c828-7891-bae3-283bafee2941` 已核 completed / error=null。接续既有 native 改动，并补 Engine 取消事实、CLI 显式共享 Host、原登记锁上的目标等批与连接层回归；所有文档由指挥同步。

- Engine：取消批次只在存在 `success=true` 或严格 JSON 布尔 `input_dispatched=true` 时提交事实；整批 invocation 配齐，保留原成功/派发结果与 sandbox fallback 诊断，Denied 仍为 Authorization，其余结果明确 Cancelled。逐条 Completed / 可选 Diagnostic → 唯一工具 MessageCommitted → Cancelled → RunCancelled，零新 Provider 请求、零重派发；无事实批次沿旧取消路径。
- 共享 Host：`chat/run --gui-host[=<SOCKET>]` 在 Core 装配前分流为已有 Host 的认证客户端，TTY / API 1.27 / workspace / session 校验及不支持参数明确拒绝；GuiServer 与 Adapter 共用 ConnectionManager。握手名仅固定 Run 的等批开关，不能签授权。request 后 bind/launch 在 Policy 与通用审批之后、scheduler 之前等待同 scope 的单一真实 requested target；grant/revoke/close 唤醒，取消有界检查，不占 native 工具 timeout。CLI 连接 ACK 游标与持久会话游标分开，Timeline 从会话 0 分页、next 直接作为 after；Ctrl-C 取消原 Run，断线观察/恢复原 Run、不重新执行。
- 模拟产品链：复用既有 EngineSurface / NativeEngineProvider，仅替代外部 Provider 与平台；真实 GuiClient → MemoryTransport → GuiServer → 同一 Core.native → request → 第二个 GUI 客户端明确 ForRun 批准 → bind → 一次 move / 一次新捕获 → Engine / SessionStore → ObservationList / Timeline。断线后 Resume 补回同一 Run 的终态，动作计数仍为 1；等批期间取消则零 move / capture 且持久 RunCancelled。既有 in-process 主/拒批两项保留，Engine 另验派发后取消事实。

实际验证（原始日志 `/tmp/pawork-cu10-glm-final/`）：`cargo test -p pawork-engine --offline --lib` 67 passed（02）；`cargo test -p pawork-cli --offline --lib` 51 passed（08）；`cargo test -p pawork-app --offline --lib services::run::tests::computer_native_wire` 3 passed（15）；同命令过滤 `computer_native_engine` 2 passed（16）；`bash scripts/test.sh computer-use` 99 passed（17）；`cargo test -p pawork-tools --offline --lib computer_native` 4 passed（18）；`cargo check -p pawork --offline` exit 0（19）。完整合法 read_thread 补存退出码，未以 grep 命中或后续 shell 命令成功替代 Cargo 退出码。验证未扩至 App 全 lib / Desktop / 全 workspace。

失败与恢复：05/07 是 CLI 首次编译失败，09/10 是连接层测试编译失败，11/12 是测试持有的 listener/Core 引用未释放，13/14 是定位诊断；修后 15 通过。15 曾重复运行并覆盖首次通过输出，当前文件记录真实重跑结果及 EXIT:0；旧输出未完整保留的边界如实登记。格式化波及的原本干净文件已核对恢复，只保留本任务差异。执行者 20-final-diff.txt 仅含 tracked diff 与 untracked 清单，不能作为完整受审内容；指挥另生成包含两个新增 Rust 文件全文的受审 diff / SHA256 清单。

本轮证据不声称执行过 CLI 私有 `run_on_gui_host` 入口、真实 Unix socket、GPUI 真窗、真实 AX / TCC / Provider 或真实用户连续输入。参数解析/拒绝与 Host wire 模拟分别证明各自范围；真实界面验收分别在 CU-18/19/20 继续模拟收口。Docker daemon 已重新核实 running，不能沿用 10-07 的“Docker 未运行”当作当前环境排除；本项连接层验证未启用专用桌面容器。原生网站身份缺信号和 pointer 三族 Unsupported 保持拒绝。

Validated: 上述定向命令与正式宿主类型检查。
Targeted regressions: 取消派发事实；共享目标等批、GUI 明确批准、持久化/查询、原 Run 恢复零重执行与取消零派发；原生台账/工具安全回归。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

独立初审：`gpt-6.1-sol max` 对话 `01a11eff-491c-7970-8588-077316a89e3f`，turn `01a11eff-4cfc-7651-b98e-b59ffde9de55` 已核 completed / error=null。完整受审 34 文件 / 610514 字节 / SHA256 `094a83e3a7580321f2f286597cbac0b63da2fb7ff19ad36f5b1cb44a1fe3dd85`，reviewer 独立重建与逐文件清单一致。Engine 取消事实、共享台账与 Host wire 模拟证据获认可；发现两项 P2：REPL 等待输入不发心跳，跨 30 秒空闲后下一轮直接请求失败；重连 / Resume / Timeline 查询期间没有连续 Ctrl-C 监听，取消意图可能丢失。原文 `/tmp/pawork-cu10-glm-final/REVIEW-R1-ISSUES.txt`，已交回原 GLM max 执行对话最小修复，并要求 CLI 自身跨空闲两轮与恢复期间取消的定向模拟。reviewer 只读、未运行 Cargo。整体尚不提交或标 ✅，修后仍由同一审查者复核。

### 初审修复与 CLI 模拟补强（2026-10-09，R2 原两项 P2 已闭合）

同一执行对话先由 GLM max 修复两项 P2，turn `01a11f0a-881f-7aa0-b1e2-8c90a12b887f` completed / error=null；用户随后将后续执行改为 `xai/grok-4.7 xhigh`，原对话以该配置续接 turn `01a11fa2-8fc7-7df2-9b8b-e0c43acc7344`，已核 completed / error=null。源码只增量修改 CLI 与必要 dev 依赖；App、Engine、computer-use、tools 的受审源码哈希未变，原定向证据继续适用。

REPL 在等输入时每 15 秒心跳，未完成的 `read_line` future 全程保留，心跳失败后下一轮先恢复连接。Ctrl-C 独立监听覆盖直播、重连退避、connect / Resume 与 Timeline 查询，恢复后向原 Run 送 RunCancel 并等真实终态；Drop 回收监听任务。监听通道关闭在当次 `recv` 内挂起，不产生取消意图；协议拒绝打印且停止自动重发，只有传输未送达才恢复后重试。测试的 80ms 心跳与 500ms 往返超时仅为注入值，生产 15s / Host 30s 未变。

两个既有 CLI 回归经真实 GuiClient → MemoryTransport → GuiServer → Core / SessionStore，只替代 stdin、传输与外部 Provider。主路径写 `fir`、等真实入站帧增长且 Provider 调用仍为零、再补 `st\n`；Provider 收到完整 `first`。空闲期心跳帧增长，服务端断连后第二轮恢复同端点，Provider 收到 `second`，共两次 Run。取消回归让完整观察任务持续运行，恢复 connect 以 Notify 明确报告已进入并挂起；此时注入取消并确认监听已消费，再恢复同一端点，得到原 Run 的 Cancelled、一次 Provider 调用及持久 Timeline 的 RunCancelled，零重执行。初稿仅整行输入、暂停观察 future 后排队取消，不能证明这两个场景，已由本次补强替代。

实际日志 `/tmp/pawork-cu10-glm-final/p2-r2/`：04 定向 `cargo test -p pawork-cli --offline --lib chat::tests` 5 passed / EXIT:0；05 格式化后 `cargo test -p pawork-cli --offline --lib` 53 passed / EXIT:0；06 `cargo check -p pawork --offline` EXIT:0。01 / 02 实际文件均为缺 `mut cancel` 的编译失败、EXIT:101；03 实际文件为修后 5 passed / EXIT:0，但本次通过结论只用 04 / 05 / 06。执行者对 01 / 03 的“未写完退出码”口述与落盘不符，指挥按真实日志纠正，不补造历史。旧 p2 目录未覆盖；其早期失败输出与 Cargo 重叠记录从合法 read_thread 保存于 ROOT-P2-COMMAND-HISTORY.json。指挥未再运行 Cargo。

Validated: 上述修后 CLI 定向测试、53 项 lib 与正式宿主检查；原其它包证据及受审源码哈希复核。
Targeted regressions: 半行跨心跳完整输入、跨空闲与断连两轮各一次 Run；实际恢复 connect 期间取消原 Run、持久终态与零重执行。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

R2 独立复审：原 Sol max 对话 turn `01a11fae-04cd-7f13-86bd-eb5bc0dbaeb5` completed / error=null，仍为 ISSUES。35 文件 / 645382 字节 / SHA256 `9b9bc2dba6cf8b76e1ac5ce16672fbfa42d5da48a64091b00485ae14a81c6d9a` 与工作区逐字节一致；原半行/空闲与实际恢复取消两项 P2 已认可闭合，仅新增一项错误分类缺口：真实 Host 的 `ServerFrame::Error` 经 Client 转成 `ClientError::Protocol`，仍落入通用发送失败分支，使明确拒绝被自动重试。`AppResponse::Error` 分支已修，但不能代替真实错误帧拒绝。原文 REVIEW-R2-ISSUES.txt 已回原 Grok xhigh 线程，要求最小分类修复与沿现有 wire 回归验证一次发送；真实平台边界继续非阻断排除，整体未提交。

R2 剩余问题修复交付：原 Grok xhigh 执行 turn `01a11fb7-11ac-7871-b5bf-245de3301a82` completed / error=null，仅增量改 chat.rs。`ClientError::Protocol` 明确打印拒绝并置已送达标记，与 `AppResponse::Error` 共用停止重发语义；超时、断连与传输未确认仍保留恢复路径。既有取消回归末尾复用同一 Host 与已 Cancelled 的原 Run，以新计数客户端实际运行 `observe_gui_run`，Host 返回真实 not_found 错误帧，200ms 观察期间取消载荷计数始终为 1；原半行与实际恢复窗口取消断言保留，没有第三项测试或第二套 harness。

实际日志 `/tmp/pawork-cu10-glm-final/p2-r3/`：01 `cargo test -p pawork-cli --offline --lib chat::tests` 5 passed / EXIT:0；02 `cargo test -p pawork-cli --offline --lib` 53 passed / EXIT:0；03 `cargo check -p pawork --offline` EXIT:0。旧日志不覆盖；由原 Sol max reviewer 复审新快照后再裁决整体完成与提交。

最终独立复审 R3：原 `gpt-6.1-sol max` 对话 turn `01a11fc0-1604-7bd3-be54-cf0ba886ca69` completed / error=null，结论 PASS。35 文件 / 649728 字节 / SHA256 `5c483c3db6b0f8e14b133bd9a441b60b3e193bcd790099f80d96fc4e3091cd01` 与工作区逐字节一致，真实协议拒绝只发送一次的剩余问题闭合；原两项 P2 和初审其它包认可保留。指挥再次核对 HEAD、完整写入集、逐文件哈希与空索引后，仅回写完成/审查元数据并按精确写入集本地提交。真实 TTY、Unix socket、GPUI、AX/TCC、Provider 未验证，明确按用户模拟授权单列；GUI 实际交互的模拟收口仍归对应 CU-18/19/20 条目。本项本轮模拟范围已完成，不表示上述环境、其它任务或发布完成。
