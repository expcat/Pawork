# Pawork 计划目录

> 更新：2026-10-12。本页只维护任务状态、链接和简介；每个任务的范围、前置、实现与验收细节写在 `docs/plan/` 的独立文档。现行能力与契约仍以 [Spec](spec/README.md) 和 [架构](architecture.md) 为准，计划不代表已实现。

## 状态与维护

- 待开发、待裁决、等待人工验收、缺环境和条件性任务均未完成；开始实施时同步任务文档和本页状态。条件性任务需先满足文档里的触发条件。
- 只有任务文档的全部完成条件满足、必要验证与真实/人工验收有证据后，才将本页对应状态改为 **✅ 完成**，并同批填写任务文档的完成日期、结果、实际命令和遗留边界。已经实现或自动检查通过但仍待人工验收的任务不标 ✅。
- ✅ 表示该任务定义的交付完成，不表示其它平台、其它任务或发布完成。已完成项保留链接，归档状态单独说明；更早的完成记录仍可从 Git 历史追溯。
- 本次迁移保留原任务编号与未闭合事项；旧条目里的“零消费者”等只作为实施前核实线索。文档迁移不重跑历史门禁，也不把功能任务标为完成。
- 2026-10-09 用户授权继续全部剩余任务，人工验收使用模拟方式执行；本轮对应状态写为 **✅ 完成（模拟验收）**，任务文档记录实际模拟输入、产品链路、结果与证据。真实账号、平台权限或系统环境无法验证的部分另记缺环境，不把模拟结果登记为真人验收或真实平台通过。本轮起始使用 `zai/glm-5.3 max`；同日用户最新指令将后续新任务及修复续接改为 `xai/grok-4.7 xhigh`，每项仍新开执行对话，独立 `gpt-6.1-sol max` review 通过后本地提交。
- 同日用户明确 CU-24 / CU-25「推进设计与模拟，系统授权部分待验」；本轮不以设计或模拟交付宣称锁屏特权组件、真实目标音频采集已启用。

## Computer use Rust 升级

目标是用户继续使用本机键鼠时，Agent 能持续在后台操作目标应用，不抢鼠标、键盘、焦点或剪贴板；用户接管同一目标时让出。LCU 作为能力与接入参照，Pawork 实现独立 Rust 引擎，沿用现有 Host / Policy / Agent loop / 持久化链路。

先完成 P0 无干扰可行性验证，再确定 P1 契约。下游按任务文档的前置顺序实施，编号不是串行执行顺序；目标占用与 Host 审批须先于生产输入接线。P2 / P3 / P4 / P5 可在契约稳定、写入集不重叠时并行，P6 收口真实验收。原生后台路线未通过时明确登记阻塞，不静默退回本机全局输入，也不用容器通过替代本机验收。

### 首阶段核心任务

| 状态 | 任务 | 阶段 | 简介 |
| --- | --- | --- | --- |
| ✅ 完成（模拟验收） | [CU-01 · macOS 本机后台操作可行性与能力基线](plan/cu-01-background-feasibility.md) | P0 | 复用原生基线与异目标并发证据；Host 在途接管、取消及撤销后 Idle 模拟与 6 条回归通过，独立 review PASS，未证明能力继续拒绝。 |
| ✅ 完成（模拟验收） | [CU-02 · 目标、观测与后台能力契约](plan/cu-02-target-contracts.md) | P1 | 当前契约及正式入口拒绝语义核验，复用同版 99 条库回归与 6 条 Host 回归；源码无改，Spec 消费顺序同步，独立复审 PASS。 |
| ✅ 已实现，自动门禁与模拟验收通过；独立 review PASS | [CU-03 · 应用发现、窗口选择与受控启动](plan/cu-03-app-window-discovery.md) | P2 | bind / launch 返回同应用多窗口的 Host 句柄，所选窗口动作后的观测精确绑定；8 条工具回归与 99 条库回归通过，系统权限另列待验。 |
| ✅ 已实现，自动回归与模拟验收通过；独立 review PASS | [CU-04 · 窗口级截图与观测坐标](plan/cu-04-window-observation.md) | P2 | 所选窗口 JPEG 解码与正式工具路由、移动后新观测和缩放后拒绝通过；真实系统权限与像素另列待验。 |
| ✅ 自动回归与模拟验收通过；独立 review PASS | [CU-05 · 有界无障碍树与元素定位](plan/cu-05-accessibility-tree.md) | P2 | 正式工具的 AX 结果、查找与拒绝链已模拟；真实 AX 探针保留原日期。 |
| ✅ 自动回归与模拟验收通过；独立 review PASS | [CU-06 · 后台 AX 语义动作](plan/cu-06-semantic-actions.md) | P2 | 正式工具保留未知效果与新观测；元素拒绝和消费句柄不重试，VS Code 截断仍拒绝签发。 |
| ✅ 自动回归与模拟验收通过；独立 review / 复审 PASS | [CU-07 · 后台定向文本与按键输入](plan/cu-07-background-text-keys.md) | P2 | 部分发送回执与退出后观测拒绝通过；真实用户连续输入另列待验。 |
| ✅ 自动回归复用与模拟验收通过；独立 review PASS | [CU-08 · 后台定向点击、滚动与拖动](plan/cu-08-background-pointer.md) | P2 | 核 AppKit 非最小化窗口移动与新观测；Pointer 未支持不提升。 |
| ✅ 自动回归复用与模拟/隔离桌面验收通过；独立 review PASS | [CU-09 · 目标占用、用户接管与后台生命周期](plan/cu-09-ownership-cancellation.md) | P3 | 真实 RFB 冲突与在途释放通过；GUI 连接/重放及接管采用同版模拟证据。 |
| ✅ 完成（模拟验收） | [CU-10 · 动作与新观测的 Host 闭环](plan/cu-10-action-observe.md) | P3 | 共享 Native Host、Engine 取消事实与 CLI 显式接入闭环完成；连接层、半行/恢复取消/拒绝单次发送模拟通过，独立 R3 review PASS，真实平台边界单列。 |
| ✅ 当前定向回归与模拟验收通过；独立 review PASS | [CU-11 · 图像能力的前置工具门控](plan/cu-11-model-vision-gate.md) | P3 | 在工具暴露和执行前拒绝不支持图像的模型，避免截屏后才失败。 |
| ✅ 当前定向回归与模拟验收通过；独立 review PASS | [CU-12 · 请求侧截图历史与上下文预算](plan/cu-12-screenshot-context.md) | P3 | 限制发给模型的旧截图，保留完整持久化证据和历史重放。 |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS | [CU-13 · 内置浏览器截图与 DOM 观测](plan/cu-13-browser-observation.md) | P4 | 在现有系统 WebView 上补截图与稳定 DOM 观测，保持用户可同时使用 Pawork。 |
| ✅ 完成（模拟人工验收）；独立 review / 复审 PASS | [CU-14 · 内置浏览器标签页与任务生命周期](plan/cu-14-browser-tabs.md) | P4 | 将每任务单页扩展为有归属的多标签，支持选择与清理。 |
| 实现与自动门禁完成；真实桥 + fake extension 模拟人工验收通过；独立 review / 复审 PASS；真实浏览器 / 系统安装 / 真实像素与竞态待环境 | [CU-15 · 外部 Chrome / Edge 的真实后台控制](plan/cu-15-external-browser.md) | P4 | MV3 扩展 + native messaging 桥（pawork_browser_bridge）+ Host external_browser 工具 + Desktop ext_ 分流；可模拟链路已验收，真实安装与浏览器验收保留待验。 |
| ✅ 完成（模拟验收） | [CU-16 · 应用与网站范围的 Host 审批](plan/cu-16-target-approvals.md) | P1 | 当前 GUI 审批到共享台账与正式工具链的 12 项定向测试通过，源码无改，独立 review PASS；原生网站缺来源仍拒绝，系统权限独立待验。 |
| ✅ 完成（Host 侧模拟验收）；独立 review PASS | [CU-17 · GUI 观测查询与协议版本接入](plan/cu-17-gui-observation-protocol.md) | P5 | 为 GUI 提供有界目标状态与截图证据查询，保留 Host 作为唯一入口；真窗消费侧验收归 CU-18/19。 |
| ✅ 完成（真窗模拟验收）；独立 review PASS；真实系统授权与原生目标待环境 | [CU-18 · GUI 目标选择与权限交互](plan/cu-18-gui-target-approvals.md) | P5 | 让用户选择后台操作目标并管理应用/网站授权。 |
| ✅ 完成（真窗模拟验收）；独立 review / 复审 PASS；真实 Provider 与原生目标待 CU-20 | [CU-19 · GUI 截图证据与停止状态](plan/cu-19-gui-evidence-stop.md) | P5 | 让用户查看实际操作前后证据，并可靠停止后台任务。 |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS；真实环境待验 | [CU-20 · macOS 后台能力端到端验收与文档收口](plan/cu-20-native-acceptance.md) | P6 | 闭合首阶段本机后台核心能力，证明用户可持续工作且操作证据完整。 |

### 后续环境与能力

以下是当前对话已规划的后续任务；平台/授权条件尚未满足，不代表已经启动或实现。Windows 同样遵守无干扰目标，用户主桌面的前台输入不作为完成方式。

| 状态 | 任务 | 阶段 | 简介 |
| --- | --- | --- | --- |
| 条件性 | [CU-21 · 隔离 Linux X11 / AT-SPI 应用控制](plan/cu-21-isolated-linux.md) | 后续环境 | 为现有专用 Linux 桌面增加 Rust 应用/窗口和元素控制通道。 |
| 条件性 | [CU-22 · Windows 独立会话 / VM 后台能力](plan/cu-22-windows-isolation.md) | 后续平台 | 通过 Rust Windows 系统 API 实现独立环境中的应用、窗口、UIA、截图和输入。 |
| 条件性 | [CU-23 · Wayland 隔离后台路线可行性](plan/cu-23-wayland-feasibility.md) | 后续平台 | 验证 Wayland 是否存在满足无干扰目标的独立桌面截图与输入路线。 |
| 设计完成；契约拒绝模拟通过；独立 review / 复审 PASS；系统安装/授权与真实锁屏待环境 | [CU-24 · macOS 锁屏使用独立设计与验证](plan/cu-24-macos-locked-use.md) | 后续能力 | 研究并验证锁屏下的受限后台操作，不把普通 AX 后台操作等同锁屏支持。 |
| 设计完成；契约拒绝模拟通过；独立 review / 复审 PASS；系统授权/真实采集与模型音频待环境 | [CU-25 · 显式授权的目标音频采集](plan/cu-25-audio.md) | 后续能力 | 补齐目标音频录制或模型使用链路，并明确二者的能力边界。 |

## 待裁决：API 与产品取舍

以下事项涉及公开 API、产品范围或依赖方向。先核实当前事实并完成具体裁决，再执行必要修改；原 B 编号保留。

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| 已裁决并实现，独立 review PASS，Windows 待验 | [B1 · Policy / Exec 公开面与安全回归裁决](plan/b1-policy-exec.md) | macOS 定向回归、Windows 交叉类型检查与依赖检查通过；Windows Job 原生执行待环境。 |
| ✅ 完成 | [B2 · Storage 预留接口与恢复面裁决](plan/b2-storage.md) | 删除未接线入口、收窄 Options 公开面，保留账本恢复读口；Storage 定向回归、宿主与契约检查通过，独立 review PASS。 |
| ✅ 完成 | [B3 · Control-plane 读面与未接线能力裁决](plan/b3-control-plane.md) | 保留真实额度读链与租约回归，删除未消费读口 / picker，收窄预算配置；定向回归与宿主检查通过，独立 review PASS，持久恢复激活与 Retention 兼容边界已写明。 |
| ✅ 完成 | [B4 · Workspace / App Prompt-template 预留裁决](plan/b4-prompt-template.md) | 删除未激活的模板字段、限制与资源变体，保留现行加载 / 导入格式；92 条定向回归通过，独立 review PASS。 |
| ✅ 完成 | [B5 · Models 四个预留 API 裁决](plan/b5-models.md) | 删除四个测试专用包装口及目录来源 trait，保留生产目录 / 单模型证据 / 用途消费；models 52 条回归与正式宿主类型检查通过，独立 review PASS。 |
| ✅ 完成 | [B6 · Auth 存储 API 与别名保留期裁决](plan/b6-auth.md) | 删除未消费的 API key 包装写删口、到期构造器与 Device 中转类型，保留 keychain_* 读 alias 并明确删除条件；Auth 82 项回归与宿主类型检查通过，独立 review PASS。 |
| ✅ 完成 | [B8 · MCP Secret 前缀单一事实源裁决](plan/b8-mcp-secret-prefix.md) | 保留 workspace 导入前缀副本，auth locator 为运行时权威；不改依赖、不物理去重，字节与文档 / 格式检查通过，独立 review PASS。 |
| ✅ 完成 | [B10 · Engine 预留入口与上下文 API 裁决](plan/b10-engine.md) | 裁决未消费入口、工具结果裁剪模块和跨 Storage 小 API。 |
| ✅ 完成 | [B11 · Orchestration 注入与恢复面裁决](plan/b11-orchestration.md) | 五个闲置入口及耦合入口收到测试构建，保留生产生命周期与重放；87 条定向回归、正式宿主类型检查通过，独立 review PASS；不激活 DAG、自动合并或热恢复。 |
| ✅ 完成 | [B12 · Git Stage / HunkStage 产品面裁决](plan/b12-git-stage.md) | 归档无消费者的 Stage/HunkStage；35 条 Git 回归、宿主与依赖检查通过，独立 review / 复审 PASS；冻结 git_stage 仍不可用，Desktop Changes 保持只读。 |
| ✅ 完成 | [B13 · MCP OAuth 未接线模块裁决](plan/b13-mcp-oauth.md) | 归档未接线的 MCP OAuth 装配；MCP 61 条回归、Auth 5 条安全锚点与宿主检查通过，独立 review / 复审 PASS；SecretRef 与独立 mcp-auth.json 不变。 |
| ✅ 完成 | [B14 · AppCore 同步装配双轨裁决](plan/b14-app-construction.md) | 删除零消费者的同步 `from_resolved`，保留异步 `from_config`；14 条装配回归与正式宿主类型检查通过，独立 review PASS。 |
| ✅ 完成 | [B15 · App 查询、设置写盘与配置错误收敛](plan/b15-app-small-cleanup.md) | DiffGet complete 恒真简化；设置重复写盘已落地，no-op；统一三处配置错误路径与跨平台文案，2 条定向回归和正式宿主类型检查通过，独立 review / 复审 PASS。 |
| ✅ 完成 | [B16 · App 子 Core 认证修订与重导出收窄](plan/b16-app-auth-revision.md) | 同 Provider 子 Core 记下所持 adapter 的认证修订；收窄 EventHub 与幂等家族公开面，32 条定向回归和正式宿主类型检查通过，独立 review PASS。 |
| ✅ 完成 | [B17 · CLI 帮助与附件检查裁决](plan/b17-cli-copy.md) | gateway 与附件帮助、合计上限文案改为中文；描述符类型复查保留。定向格式、链接与静态差异检查通过，独立 review PASS；未执行真实 help 或附件运行时。 |
| ✅ 完成 | [B18 · Client SDK MockTransport 与版本 API 裁决](plan/b18-client-sdk.md) | 保留四个 MockTransport 公开辅助方法与 `sdk_version_string`；headless rustdoc 的协议当前版本改为 1.27。定向静态、链接、格式与 diff 检查通过，独立 review / 复审 PASS；未改行为，未跑 Cargo。 |
| ✅ 完成 | [B19 · Desktop 终端响应与实例路径裁决](plan/b19-desktop-terminal-paths.md) | create 回执只保留 terminal_session_id；删除无生产调用的 socket/token wrapper。三项定向回归通过，独立 review / 复审 PASS；未执行真窗口或历史二进制验收。 |
| 已实现；定向回归与独立 review / 复审 PASS；真实探针验收待完成 | [B20 · Desktop Probe 功能测试模型裁决](plan/b20-desktop-probe-models.md) | --probe 只列目录；smoke 当次固定 opencode-go / glm-5.3-flash。唯一授权模型覆盖不了切换，输出 model_switch=uncovered。真实探针未跑，独立 review 不能关闭。 |
| 已裁决保留；静态检查与独立 review / 复审 PASS；真实 GUI 未验收 | [B21 · Desktop 大小格式与词条入口裁决](plan/b21-desktop-format-i18n.md) | 保留两套 B/KiB/MiB 格式、单工具成功 headline 与 task_usage 本地 tr。0 到 16 MiB 的逐值比较显示串一致；未改运行行为、不新增词条，原真实验收条件保留。 |

条件性工程项 RV-2026-10-A / RV-2026-10-C 的触发条件仍见 [backlog](spec/backlog.md#8-条件性工程项)。RV-2026-10-B 已由 B8 裁决关闭：保留导入副本，不物理去重。B10 对 RV-2026-10-A 的交叉说明仍在其任务文档。

## 等待人工验收

相关功能的历史实现与自动门禁结论承接迁移前 ROADMAP；用户验收和外部消费者接入独立跟踪，本次没有重新执行产品验证。任务消耗的 Pawork、MoMai、YingMai 验收已拆开。

### 已完成（模拟验收）

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS；真实窗口与系统文件选择器待验 | [RV-01 · 图片附件交互人工验收](plan/rv-01-image-attachments.md) | GPUI 测试窗口验证有效 1×1 PNG 缩略图解码、预览 / 关闭 / 移除键盘分离，以及切任务与迟到发送失败时的文本和附件归属。 |
| ✅ 完成（模拟验收）；独立 review / 多轮复审 PASS；真实窗口与系统文件选择器待验 | [RV-02 · 文本附件与不可信边界人工验收](plan/rv-02-text-attachments.md) | Host 包装、正文折叠、正式 AX Press、重复事件 Replay、历史页重载与含标记助手负例的结构一致性模拟通过。 |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS；真实窗口与系统文件选择器待验 | [RV-03 · Composer 附件错误人工验收](plan/rv-03-composer-errors.md) | 真实临时文件的允许 / 类型拒绝 / 大小拒绝 / 清错 / 数量上限、中英文案与 AX、草稿焦点及修正入口在 GPUI 测试窗口模拟通过。 |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS；真实 Host 现场重放与窗口待验 | [RV-04 · 子代理工具展开人工验收](plan/rv-04-subagent-tools.md) | 历史工具事实水合后的目标、参数、回执、折叠稳定性与拒绝 / 失败区分在 GPUI 测试窗口模拟通过。审批标题按 Host 字段回退为 approval requested · tool · tool `write_file` requires approval，拒绝为 approval deny。 |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS；真实窗口与像素待验 | [RV-05 · 子代理回执阅读人工验收](plan/rv-05-subagent-receipts.md) | Markdown 回执与主正文同源渲染，长回执折叠 / 展开 / 再折叠、外层滚动与 2000 字符前缀去重边界在 GPUI 测试窗口模拟通过。 |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS；真实窗口与像素待验 | [RV-06 · 子代理标签字号人工验收](plan/rv-06-subagent-font-sizes.md) | 100% / 125% / 150% 字号与侧栏 / 中央布局下，超宽标题换行、chip 宽度钳制、状态点与焦点环完整由产品自行收敛；GPUI 测试窗口模拟通过。 |
| ✅ 完成（模拟验收）；独立 review / 多轮复审 PASS；原生 AX bridge、VoiceOver、真窗口与像素待验 | [RV-07 · 子代理正文键盘与朗读人工验收](plan/rv-07-subagent-accessibility.md) | 正文与展开按钮共用 GPUI 焦点句柄并进入 Tab 链；AX Focus、Enter 与 Press 走正式路径，离屏焦点由产品滚入视口，同帧滚底覆盖后可重试。 |
| ✅ 完成（模拟验收）；独立 review / 复审 PASS；真实 Host、网络、Provider 与窗口待验 | [RV-11 · 子代理断线与恢复人工验收](plan/rv-11-subagent-reconnect.md) | 断线保留代理列表、选中项、正文与阅读位置并作废在途代际；同会话重连保留内容，权威列表移除原代理时收敛选中，切会话才清空。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实 Host、真窗口、系统缩放与像素待验 | [GUI2-01 · 工作台壳、首页与侧栏人工验收](plan/accept-gui-workbench.md) | 首页新建、项目 / 无项目切换、侧栏分组折叠、草稿与选择保持、连接状态层级经真实投影 / AX / 键盘路径模拟通过。 |
| ✅ 完成（模拟验收）；独立 review 初审 P2 修复后复审 PASS；真实 Host、真窗口、系统 IME 与像素待验 | [GUI2-02 · 对话阅读层级与 Composer 人工验收](plan/gui2-02-conversation-composer.md) | 消息阅读层级、代码 / 表格块内横滚与原文复制、消息菜单三种输入路径、模型 / 供应商管理入口、Settings 往返保持与三档字号 Composer 几何经 GPUI 主路径模拟通过。 |
| ✅ 完成（模拟验收）；独立 review 初审 P2 / 复审 P3 修复后最终 PASS；真实 Host、真窗口、原生 AX 与像素待验 | [GUI2-03 · 任务与操作快捷查找人工验收](plan/gui2-03-quick-search.md) | 侧栏与 Cmd+K 打开、当前 Snapshot 任务 / 页面查找、方向键 / Enter / Esc、草稿隔离、无结果与断线 gate 经 GPUI / AX 路径模拟通过；断线范围文案补齐本地浏览器。 |
| ✅ 完成（模拟验收）；独立 review 初审 3 项 P2 修复后复审 PASS；真实 Host、真窗口、原生 AX、系统按键像素与真实历史分页待验 | [GUI2-04 · 当前对话查找与回合定位人工验收](plan/gui2-04-conversation-navigation.md) | 当前已加载正文查找、命中循环 / 无结果、长对话回合目录滚动定位、流式转持久身份、阅读模式解锚与切任务清空经 GPUI / AX 路径模拟通过。 |
| ✅ 完成（模拟验收）；独立 review 初审 P2 修复后复审 PASS；真实 Host、真窗口、原生 AX bridge、真实像素、PTY、浏览器与文件系统事实待验 | [GUI2-05 · 工作面板与窄窗可达性人工验收](plan/gui2-05-work-panels.md) | 宽窗并排与窄窗中央呈现、Changes / Terminal / Resources 往返保持、既有面板错误 / 空态与文件 / 浏览器路径经 GPUI / AX 路径模拟通过。 |
| ✅ 完成（模拟验收）；初审 P2 / P3 修复后独立复审 PASS；真实平台边界待验 | [GUI2-06 · 设置查找与行式信息组织人工验收](plan/gui2-06-settings-search.md) | 中英查找、定位与清空、导航几何、控件焦点 / pending / stale 及供应商错误分层经 GPUI / AX 模拟验证；测试语言线程隔离、复用产品滚动方法，修后五条回归与生产分支 check 通过。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验 | [GUI2-07 · 受影响主路径与旧缺口收口验收](plan/gui2-07-main-path-acceptance.md) | 复用 GUI2-01～06 已审证据；跨界草稿 / 查找 / Changes / Settings / 迟到回执回归通过，修复非 Workspace Enter 误发隐藏草稿。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验 | [GUI3-01 · 工具调用可扫读人工验收](plan/gui3-01-tool-summaries.md) | 历史工具投影、摘要状态计数、鼠标 / Enter / Space / AX 展开与 10 行预览 / 全文回归通过；生产行为未改，真实平台边界待验。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验 | [GUI3-02 · Composer 元信息与模型入口人工验收](plan/gui3-02-composer-metadata.md) | 四条既有回归补断言、六条定向测试通过；Context 缺数据、项目归属与首页、模型入口及取消不伪造终态的边界模拟完成。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验 | [GUI3-03 · 代码块头、链接与表格人工验收](plan/gui3-03-markdown-content.md) | 语言标签、Code 回退、HTTP(S) 行内与菜单打开 / 复制、未知协议不分发通过；长表 / CRLF 复用同版已审证据。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验 | [GUI3-04 · 失败卡与下一步人工验收](plan/gui3-04-run-failure-next-step.md) | 三档长原因、认证导航与 Review 条件 / 正式点击通过；文件清单为测试投影，真实 Host / Git 待验。 |
| ✅ 完成（真窗模拟验收）；初审 P2 修正后独立复审 PASS；真实 Provider 待验 | [GUI3-05 · SVG 图标与语义颜色人工验收](plan/gui3-05-svg-icons.md) | 三档图标像素、Send / Cancel / 焦点、代表 hover / tooltip / 按住输入与归档回执通过；Spec 旧尺寸与形状同步，Ghost 色差边界保留。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验 | [GUI3-06 · 侧栏降噪与字号密度人工验收](plan/gui3-06-task-rail-density.md) | 正式 live 投影、分组及 150% 改名 / 归档模拟通过；普通标题三档真窗复用同版证据，长标题只核模拟布局。 |
| ✅ 完成（真窗口与 GPUI 模拟验收）；初审修正后独立复审 PASS；系统边界待验 | [GUI3-07 · Header 动作与断线层级人工验收](plan/gui3-07-header-recovery.md) | 两级重连、草稿保持及 Activity → Changes 通过；导航复用已审模拟回归，焦点为手动回焦，系统按键 / 悬停像素待验。 |
| ✅ 完成（模拟验收）；初审 P2 修复后独立复审 PASS；真实系统动效待验 | [GUI3-08 · 层次 token 与有限动效人工验收](plan/gui3-08-surfaces-motion.md) | 修复 Switch 同值重绘跳动；Inspector 动态、两个面板独立骨架与流式标记的正式 GPUI 绘制记录通过。 |
| ✅ 完成（模拟验收）；初审 P2 修复后独立复审 PASS；系统边界待验 | [GUI4 · StatusBar、空态与终端人工验收](plan/gui4-status-empty-terminal.md) | 真实 Host / PTY 主路径、双标签输出隔离与状态栏回归通过；空态复用已审证据，系统 IME 与真窗口像素待验。 |
| ✅ 完成（模拟验收）；初审 P2 引用修正后独立复审 PASS；系统边界待验 | [AC-02 · IME、模型目录与跨面板交互人工验收](plan/accept-ime-model-navigation.md) | 换行后组合输入、目录搜索 / 开关回执及字号错误共存通过；布局与跨面板复用已审证据。 |
| ✅ 完成（模拟验收）；独立 review PASS；系统边界待验 | [AC-03 · 产品 Plan 窗口键盘人工验收](plan/accept-plan-keyboard.md) | 三字段、五动作、busy / 未保存 / 旧 Host 与只读回执、两窗口及 Composer 父焦点模拟通过。 |
| ✅ 完成（模拟验收）；独立 review PASS；真实 Provider 与非 Unix 待验 | [AC-04 · 技能录制产物后续 Run 发现验收](plan/accept-skill-reuse.md) | 正式保存后两次后续 Run 的技能正文、持久注入与终态通过，撤销信任后不注入；隔离复跑与既有拒绝回归通过。 |

### 待验收

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| ✅ 完成（模拟验收）；独立 review PASS；原生系统与消费者分别待验 | [AC-05 · Pawork 任务消耗窗口人工验收](plan/accept-task-usage.md) | 临时 SQLite 101 条调用经正式 Host 查询进入 GPUI 测试窗口，汇总、筛选、分页、未知值和刷新查询失败通过。 |
| 等待人工验收 | [AC-06 · 模型用途筛选人工验收](plan/accept-model-purpose.md) | 验收 Composer 用途菜单和 Settings 能力徽标。 |
| 等待消费者接入与人工验收 | [AC-07 · MoMai 网关凭证与媒体消费者验收](plan/accept-gateway-consumers.md) | 闭合 MoMai 设置保存凭证及媒体 GUI 验收。 |
| 等待消费者接入与人工验收 | [AC-08 · MoMai 任务消耗消费者接入与验收](plan/accept-momai-task-usage.md) | 闭合 MoMai 作品、分部和章节三个维度的消费者接入。 |
| 等待消费者接入与人工验收 | [AC-09 · YingMai 任务消耗消费者接入与验收](plan/accept-yingmai-task-usage.md) | 闭合 YingMai 作品、分段和操作类型三个维度的消费者接入。 |

## 缺环境与外部条件

在前置条件具备后验证真实效果；mock 或一般本机测试不能替代对应环境。平台实现缺口与仅缺环境的情况在各任务文档中分别说明。

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| 缺环境 | [ENV-01 · 真实账号与额度环境验收](plan/env-01-accounts-quota.md) | 补齐 OAuth、刷新、双账号与权威额度的真实环境证据。 |
| 缺环境 | [ENV-02 · 既有跨平台专项环境验收](plan/env-02-platform-matrix.md) | 补齐 Linux / Windows、WebKit、容器和真实 Provider 的既有专项矩阵。 |
| 缺环境 | [ENV-03 · MM-2 视频真实往返验收](plan/env-03-video-roundtrip.md) | 在支持的视频端点与账号上证明真实往返。 |
| 缺环境 | [ENV-04 · MM-3 GLM 搜索环境验收](plan/env-04-glm-search.md) | 配置真实 GLM 搜索 MCP 后验收搜索链路。 |
| 缺浏览器授权 | [ENV-05 · Chrome / Edge 只读上下文环境复验](plan/env-05-browser-context.md) | 用户手动授权后复验当前页正文读取。 |
| ✅ 完成（维持 Unsupported 裁决分支） | [ENV-06 · 非 Unix 技能录制写入前置与验收](plan/env-06-skill-non-unix.md) | Host 固定平台拒绝提示、两条既有 macOS 回归与独立 review PASS；不新增非 Unix 写入、不冒充平台支持或原生验收。 |
| 缺环境 | [ENV-07 · 网关媒体供应商与账单环境验收](plan/env-07-media-billing.md) | 验收真实图像、视频、搜索供应商、套餐权限和账单来源。 |

库激活前置、按测量触发的技术项和阶段外产品候选仍见 [backlog](spec/backlog.md)。发布与全量门禁维持另行授权（BK-RELEASE-01），不因任务文档或上述能力收口而视为完成。
