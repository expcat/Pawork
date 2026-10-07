# CU-20 · macOS 后台能力端到端验收与文档收口

> 状态：自动回归通过；CU-06/07/08 探针动作验收通过，CU-14 标签条滚动真窗复验完成，回放核对零重执行（2026-10-08）；GUI 模型链路被用户配置 fail-closed 拒绝（行为正确、环境阻塞），CU-18 授权回执 / CU-19 观测行与停止状态机 / CU-15 扩展安装、并发无干扰（用户连续输入）与真实接管让出待人工验收。前置：[CU-03](cu-03-app-window-discovery.md)、[CU-04](cu-04-window-observation.md)、[CU-05](cu-05-accessibility-tree.md)、[CU-06](cu-06-semantic-actions.md)、[CU-07](cu-07-background-text-keys.md)、[CU-08](cu-08-background-pointer.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-10](cu-10-action-observe.md)、[CU-11](cu-11-model-vision-gate.md)、[CU-12](cu-12-screenshot-context.md)、[CU-13](cu-13-browser-observation.md)、[CU-14](cu-14-browser-tabs.md)、[CU-15](cu-15-external-browser.md)、[CU-16](cu-16-target-approvals.md)、[CU-17](cu-17-gui-observation-protocol.md)、[CU-18](cu-18-gui-target-approvals.md)、[CU-19](cu-19-gui-evidence-stop.md)。更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

闭合首阶段本机后台核心能力，证明用户可持续工作且操作证据完整。

## 当前依据与读取范围

（2026-10-06 规划背景）规划时只有静态研究和规划，没有新引擎实现、真实窗口或 Provider 验收；旧隔离桌面和历史 macOS 原型结论不能替代本机后台证据。实际验收范围与结果见「完成记录」。

[验证规格](../spec/verification.md)、[computer-use Spec](../spec/crates/computer-use.md)、[browser Spec](../spec/crates/browser.md)、[架构](../architecture.md)、[GUI 设计](../gui-design.md)。

## 实施范围

- 先跑实际写入集的既有定向测试，再在真实 AppKit / Electron / 浏览器完成应用定位、截图、元素/键鼠和多应用任务。
- 用户同时在其它应用连续输入，核对目标 UI/文件/DOM 与用户侧文本及焦点/指针；纳入遮挡、接管、取消、过期、拒绝和恢复。
- 真实模型固定临时参数 `--provider opencode-go --model glm-5.3-flash`，不写默认；连接或图像能力失败如实记录，不换模型凑通过。
- 回放和 GUI 同时核对原始证据，不重执行动作。只有出现新变化或未解决问题才扩大测试，不跑无关全 workspace。
- 按实际改动同步包级 Spec、architecture/design/gui-design 与本任务的能力矩阵，分别写实现、自动验证、真实/人工验收；发布仍非本任务。

## 完成条件与验证

- 核心场景满足不抢键鼠/焦点/剪贴板、持续后台执行和用户接管让出，所有通过结论有真实证据。
- 未支持动作列为缺口；若核心目标未达到，不得仅凭子任务或隔离环境通过把本项标 ✅。
- GUI 证据、Host 状态、持久化/恢复与实际目标结果一致，用户验收结果明确。
- 后续平台/锁屏/音频任务仍独立跟踪，本项完成不表示全平台或发布完成。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

实现（2026-10-08，写入集 = 探针修复 + 文档收口，未提交）：`crates/computer-use/examples/macos_target_probe.rs` 修复 CU-17 审查引入的 open_scope 要求导致的探针失效——`probe_registry()` 建表即登记活跃 scope（5 处调用点），`register_ax_observation` 错误附窗口 bounds 调试上下文；无产品代码改动，能力矩阵零变化。

实际命令与定向回归：`bash scripts/test.sh computer-use browser app`（7 目标 426 项全绿：app 302、browser 25、computer-use 99）；`bash scripts/test.sh desktop`（274 全绿）；探针构建与全部验收命令经实际执行验证（`macos_target_probe` 的 axact / axinput / axpointer / axmove / axminimize / axunminimize / axsetframe / wins / launch / open / quit 序列，逐项证据 JSON tee 至 /tmp/cu20/、不入库）。

真实结果（2026-10-08，macOS 26.6.2 arm64 本机，控制台已解锁，用户在机器上持续工作——动作步采样 front / focus 恒为 Edge pid 916、鼠标有真实活动）：

- 不抢键鼠 / 焦点 / 剪贴板：CU-06/07/08 全部动作步前后采样 front_unchanged / focus_unchanged / clipboard_unchanged，鼠标零漂移（用户自发移动如实记录）；逐件证据见 CU-06 / CU-07 / CU-08 完成记录，本段不重复粘贴。
- 持续后台执行：AppKit 语义写与定向文本键、Browser 按压均后台落地，目标窗口全程未前台。
- 用户接管让出：验收期间用户在步骤间点击过 TextEdit 窗口、VS Code 启动后曾自激活前台一次（vsc-open.json 如实记录），随后均回到 Edge；所有动作步逐步采样前台恒为用户应用，接管期间无后台派发落入目标。接管让出本身未经真实派发路径验证——探针直驱 registry、未经 pause_dispatch / UserActive 闸，UserActive 语义仅契约与定向回归通过（CU-09 真实接管信号待接线），真实接管让出登记待人工验收；用户连续输入并发探针同样待人工（sessiontype 会向真实前台发事件，见 CU-07 遗留缺口）。
- 遮挡 / 取消 / 过期 / 拒绝 / 恢复：目标窗口全程后台被遮挡照常落地；取消与过期由 CU-06/07/08 定向回归钉住；拒绝全程真实（能力闸 / ElementUnsupported / 最小化闸 / UnprovenUnique）；恢复真实——axinput 首次 TreeChanged fail-closed、重新观测签发后成功。
- 回放核对（2026-10-08，实例 cu14 = Host `--instance cu14 gui serve` + Pawork-cu14.app）：打开 CU-14 旧会话，时间线完整渲染 browser 工具组与模型汇报（tab-47438-4/5 归属、select_tab 切回、close 回收），/tmp/cu14/http.log 保持 9 行、shasum 7808ad76… 不变——历史重放零重执行；回放期间 fixture 服务在线（127.0.0.1:8932）以捕获任何重执行请求。
- GUI 侧：后台操作页 no_run 态真窗复验（「当前没有运行中的 run」，AX help "ready · 0 targets"）；CU-14 标签条滚动复验通过（垂直滚轮经 GPUI overflow_x_scroll 映射横向、AX 裁剪发布、「+」点击建标签、关标签落位——详见 CU-14 完成记录）。
- 模型链路：GUI RunStart 固定 `--provider opencode-go --model glm-5.3-flash`，被用户全局配置 disabled_models（opencode-go 含 glm-5.3-flash）按 ADR-055 D4 fail-closed 拒绝——「模型 glm-5.3-flash（provider opencode-go）已被禁用」；行为正确，不换模型、不改用户配置、不伪造通过。CU-18 授权回执端到端与 CU-19 观测行 / 停止状态机需活动 run，随模型链路恢复后人工验收。

新发现登记（缺口）：Chrome 154 网页 AX 树物化远超 CU-05 Electron 的 1–2 分钟（AXManualAccessibility 对 app / 窗口元素均 -25205、AXEnhancedUserInterface -25208，约 15 分钟间歇 AX 活动后自行物化，chrome-axtree.json → chrome-axtree2.json）；探针 axtree 观测「图像 = 窗口点 1:1」口径遇 1366pt 高超窗 MAX_IMAGE_EDGE=1280 按 Invalid 拒绝（产品截图路径不受此限：图像边长由 JPEG 预算独立钳制，观测几何分记图像像素与窗口点）；VS Code 树深度截断阻断元素级句柄签发（UnprovenUnique fail-closed 正确，结构性，随 CU-05 树边界演进再议）。

遗留缺口：GUI 模型链路端到端（disabled_models 环境阻塞，待用户调整配置或授权后人工验收）；CU-15 扩展安装待用户当面确认；真实接管让出与用户连续输入并发探针待人工；锁屏场景沿用 CU-01 口径不记为支持；隔离桌面 RFB 验收不在本轮范围（CU-09 / CU-10 跟踪）；发布非本任务。核心目标中「不抢键鼠 / 焦点 / 剪贴板、持续后台执行」已获真实证据闭合；「用户接管让出」UserActive 契约回归通过、真实接管让出待人工验收；模型驱动 GUI 链路未通，按本任务完成条件不标 ✅。
