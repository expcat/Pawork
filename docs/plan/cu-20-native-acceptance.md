# CU-20 · macOS 后台能力端到端验收与文档收口

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS。真实 Provider 成功链路、真实连续输入 / OS 接管、真实 Chrome/Edge 安装像素与系统授权待环境。前置：[CU-03](cu-03-app-window-discovery.md)、[CU-04](cu-04-window-observation.md)、[CU-05](cu-05-accessibility-tree.md)、[CU-06](cu-06-semantic-actions.md)、[CU-07](cu-07-background-text-keys.md)、[CU-08](cu-08-background-pointer.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-10](cu-10-action-observe.md)、[CU-11](cu-11-model-vision-gate.md)、[CU-12](cu-12-screenshot-context.md)、[CU-13](cu-13-browser-observation.md)、[CU-14](cu-14-browser-tabs.md)、[CU-15](cu-15-external-browser.md)、[CU-16](cu-16-target-approvals.md)、[CU-17](cu-17-gui-observation-protocol.md)、[CU-18](cu-18-gui-target-approvals.md)、[CU-19](cu-19-gui-evidence-stop.md)。更新：2026-10-10。

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

真实结果（2026-10-08，macOS 26.6.2 arm64 本机，控制台已解锁，用户在机器上持续工作；2026-10-10 复核 /tmp/cu20 49 个 JSON 后收敛口径：多数动作步前台为 Edge，但 te-move.json 前台为 Codex、vsc-open.json 前台为 VS Code；鼠标有真实活动，不能把聚合采样写成零漂移）：

- 不抢键鼠 / 焦点 / 剪贴板：现有探针没有出现目标窗口为抢前台/焦点而派发全局输入的证据；24/49 个文件明确采样 clipboard_unchanged，其余文件缺剪贴板字段；25 个文件含 mouse_delta 字段，其中 11 个非零、14 个为零（用户或采样活动不能反推为目标派发，也不能写成零漂移）。逐件证据见 CU-06 / CU-07 / CU-08 记录。
- 持续后台执行：AppKit 语义写与定向文本键、Browser 按压均后台落地，目标窗口全程未前台。
- 用户接管让出：验收期间用户在步骤间点击过 TextEdit 窗口、VS Code 启动后曾自激活前台一次（vsc-open.json 如实记录），随后多数采样回到 Edge；探针直驱 registry，未经 pause_dispatch / UserActive 闸，不能作为真实接管让出证据。UserActive / pause_dispatch / Idle 恢复 / 取消语义由 2026-10-10 六条定向回归复验（见下文），真实连续输入与 OS 接管仍待环境。
- 遮挡 / 取消 / 过期 / 拒绝 / 恢复：目标窗口在后台被遮挡时语义写、文本键与 Browser 按压可落地；取消与过期由 CU-06/07/08 定向回归钉住；能力闸 / ElementUnsupported / 最小化闸 / UnprovenUnique 拒绝真实。现存 /tmp/cu20 JSON 不含 TreeChanged 字符串，不能作为「首次 TreeChanged 拒绝后恢复」的原始证据；恢复语义以定向回归与后续重新观测签发为准。
- 回放核对（2026-10-08，实例 cu14 = Host `--instance cu14 gui serve` + Pawork-cu14.app）：打开 CU-14 旧会话，时间线完整渲染 browser 工具组与模型汇报（tab-47438-4/5 归属、select_tab 切回、close 回收），/tmp/cu14/http.log 保持 9 行、shasum 7808ad76… 不变——历史重放零重执行；回放期间 fixture 服务在线（127.0.0.1:8932）以捕获任何重执行请求。
- GUI 侧：后台操作页 no_run 态真窗复验（「当前没有运行中的 run」，AX help "ready · 0 targets"）；CU-14 标签条滚动复验通过（垂直滚轮经 GPUI overflow_x_scroll 映射横向、AX 裁剪发布、「+」点击建标签、关标签落位——详见 CU-14 完成记录）。
- 模型链路：GUI RunStart 固定 `--provider opencode-go --model glm-5.3-flash`，被用户全局配置 disabled_models（opencode-go 含 glm-5.3-flash）按 ADR-055 D4 fail-closed 拒绝——「模型 glm-5.3-flash（provider opencode-go）已被禁用」；行为正确，不换模型、不改用户配置、不伪造通过。2026-10-10 隔离 Host 重发 run_start 得到同一文案的 wire error（code internal、retryable false、无 Accepted），证明协议拒绝可见。

新发现登记（缺口）：Chrome 154 网页 AX 树物化远超 CU-05 Electron 的 1–2 分钟（AXManualAccessibility 对 app / 窗口元素均 -25205、AXEnhancedUserInterface -25208，约 15 分钟间歇 AX 活动后自行物化，chrome-axtree.json → chrome-axtree2.json）；探针 axtree 观测「图像 = 窗口点 1:1」口径遇 1366pt 高超窗 MAX_IMAGE_EDGE=1280 按 Invalid 拒绝（产品截图路径不受此限：图像边长由 JPEG 预算独立钳制，观测几何分记图像像素与窗口点）；VS Code 树深度截断阻断元素级句柄签发（UnprovenUnique fail-closed 正确，结构性，随 CU-05 树边界演进再议）。

## 2026-10-10 可模拟范围收口

本轮无源码改动，证据 `/tmp/pawork-cu20-native-acceptance-simulation/evidence/`：

- **真实协议拒绝**：隔离 `pawork --instance cu20sim --provider opencode-go --model glm-5.3-flash --trust-workspaces gui serve` 握手 API 1.27 accepted 后，`run_start` 返回 `type=error`、`code=internal`、`retryable=false`，文案「模型 glm-5.3-flash（provider opencode-go）已被禁用」，无 Accepted，因此未启动 run。第一次探针只等 response 而错过 error（exit 1，保留 `disabled-model-probe.log`）；修正后的 `/tmp` 探针读取 error/response，`disabled-model-retry.log` 为最终证据。用户配置未改。
- **接管 / 连续输入模拟**：`cargo-takeover.log` exit 0。computer-use 3 项（`user_takeover_pauses_dispatch_without_consuming`、`cancellation_precedes_consumption`、`pre_flight_rejects_a_taken_over_application_before_any_probe_call`）与 tools 3 项（UserActive 保持 Once 且缺凭证拒绝、Operating 暂停 / Idle 只清暂停、取消不暂停且 Idle 不恢复撤销）通过。ActivityProof 是 Fake，不证明真实用户键入或 OS 接管。
- **主机构建**：`cargo-build-pawork.log` exit 0，dev profile 4m52s，仅既有 warning。
- **GUI 授权 / 观测 / 停止复核**：CU-18 的 approve/revoke 快照、CU-19 的停止后 active_runs=[] / targets=[] 与观测行报告仍在，哈希见 evidence/report.txt；分类为真窗模拟，不是真实 Provider 或原生目标。
- **CU-15 边界**：既有 fake extension flow/restart/conclusion 复核一致，`real_chrome_or_edge=false`、GPUI 未启动、截图为夹具；真实安装、DOM 像素与竞态待环境。
- **历史探针复核**：/tmp/cu20 49 个 JSON 只读复核。不能写成全程 Edge、零鼠标漂移或 TreeChanged 拒绝恢复；te-move 前台为 Codex、vsc-open 前台为 VS Code；25 个文件含 mouse_delta 字段（11 个非零、14 个为零）；24 个文件明确 clipboard_unchanged，另 25 个缺字段；无 TreeChanged 原文。rejected 14、dispatched 11、no_match 4。

遗留缺口：真实 Provider 成功作答链路（disabled_models 环境阻塞）；真实用户连续输入与 OS 接管信号；CU-15 真实 Chrome/Edge 安装、像素与竞态；系统授权与原生目标；CU-19 停止瞬时态 / 确认失败 / 结果未知；锁屏与发布。按用户 2026-10-09 授权，人工验收以模拟方式执行，本任务可在独立 review 通过后标「✅ 完成（模拟验收）」，以上真实环境项不冒充已通过。

独立 review（2026-10-10，Sol max，thread `01a12517-e148-71c3-a2b4-1f9305eddece`）：R1 初审 turn `01a12517-e46b-7e43-ae6a-0026db6df268` 报 1 个 P2——mouse_delta 统计把「25 个含字段」误写成 25 个非零。修正为 25 个含字段、11 个非零、14 个零后，R2 快照 13,404 字节、SHA256 `ee16c6d9e57539af17ed6d8cd85c00944082b11543edbfefa19ee98e21cdcdfe`；复审 turn `01a1251e-dcdc-7810-af43-47d956658e46` PASS，确认无新增 P1/P2/P3。执行者为 Grok xhigh thread `01a12501-9ff7-7e63-9345-d2a56fc591e6`，本轮无源码改动。
