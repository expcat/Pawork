# Pawork 计划目录

> 更新：2026-10-09。本页只维护任务状态、链接和简介；每个任务的范围、前置、实现与验收细节写在 `docs/plan/` 的独立文档。现行能力与契约仍以 [Spec](spec/README.md) 和 [架构](architecture.md) 为准，计划不代表已实现。

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
| 等待人工验收 | [CU-01 · macOS 本机后台操作可行性与能力基线](plan/cu-01-background-feasibility.md) | P0 | 证明用户继续操作本机时，Agent 能在后台观察和操作目标应用；先取得真实支持边界，再进入原生重构。 |
| 等待人工验收 | [CU-02 · 目标、观测与后台能力契约](plan/cu-02-target-contracts.md) | P1 | 在现有 computer-use / tools 内定义应用、窗口、元素和输入的最小有类型契约。 |
| 等待人工验收 | [CU-03 · 应用发现、窗口选择与受控启动](plan/cu-03-app-window-discovery.md) | P2 | 让 Agent 选择已授权的真实应用与窗口，并按目标身份受控启动应用。 |
| 等待人工验收 | [CU-04 · 窗口级截图与观测坐标](plan/cu-04-window-observation.md) | P2 | 提供目标窗口截图与可供动作使用的新鲜观测。 |
| 等待人工验收 | [CU-05 · 有界无障碍树与元素定位](plan/cu-05-accessibility-tree.md) | P2 | 为支持的目标应用提供 AX 状态、元素查找和观测绑定的句柄。 |
| 已实现；自动回归与真实验收通过（2026-10-08 探针路径三族）；VS Code 元素级受树深度截断结构性阻塞，生产接线属 CU-10 | [CU-06 · 后台 AX 语义动作](plan/cu-06-semantic-actions.md) | P2 | 优先用目标元素的语义动作实现后台点击、选择和赋值。 |
| 已实现；自动回归与探针动作验收通过（2026-10-08）；并发无干扰验收（用户连续输入）待人工，生产接线属 CU-10 | [CU-07 · 后台定向文本与按键输入](plan/cu-07-background-text-keys.md) | P2 | 补齐语义赋值不能覆盖但已证实支持后台输入的文本与按键动作。 |
| 已实现；自动回归与真实验收通过（2026-10-08 探针路径三族）；生产接线属 CU-10 | [CU-08 · 后台定向点击、滚动与拖动](plan/cu-08-background-pointer.md) | P2 | 实现已通过无干扰验证的窗口定向指针动作。 |
| 等待人工验收 | [CU-09 · 目标占用、用户接管与后台生命周期](plan/cu-09-ownership-cancellation.md) | P3 | 把现有全局串行改成明确目标归属，保证并发冲突、取消和后台生命周期可解释。 |
| ✅ 完成（模拟验收） | [CU-10 · 动作与新观测的 Host 闭环](plan/cu-10-action-observe.md) | P3 | 共享 Native Host、Engine 取消事实与 CLI 显式接入闭环完成；连接层、半行/恢复取消/拒绝单次发送模拟通过，独立 R3 review PASS，真实平台边界单列。 |
| 自动回归通过，等待人工验收 | [CU-11 · 图像能力的前置工具门控](plan/cu-11-model-vision-gate.md) | P3 | 在工具暴露和执行前拒绝不支持图像的模型，避免截屏后才失败。 |
| 自动回归通过，等待人工验收 | [CU-12 · 请求侧截图历史与上下文预算](plan/cu-12-screenshot-context.md) | P3 | 限制发给模型的旧截图，保留完整持久化证据和历史重放。 |
| 已实现；自动回归通过，read/click/句柄/拒绝/重放真实验收通过，截图正路径与焦点探针待人工验收 | [CU-13 · 内置浏览器截图与 DOM 观测](plan/cu-13-browser-observation.md) | P4 | 在现有系统 WebView 上补截图与稳定 DOM 观测，保持用户可同时使用 Pawork。 |
| 已实现；自动回归通过，归属/负路径/重放真实验收通过；复审修复四项过自动门禁，标签条滚动与空标签地址栏真窗复验通过（2026-10-08），Agent 不动展示复验待模型链路人工验收 | [CU-14 · 内置浏览器标签页与任务生命周期](plan/cu-14-browser-tabs.md) | P4 | 将每任务单页扩展为有归属的多标签，支持选择与清理。 |
| 实现完成；自动门禁全绿，真实浏览器验收待人工执行（验收机已解锁 2026-10-08，扩展安装须用户当面确认） | [CU-15 · 外部 Chrome / Edge 的真实后台控制](plan/cu-15-external-browser.md) | P4 | MV3 扩展 + native messaging 桥（`pawork_browser_bridge`）+ Host `external_browser` 工具 + Desktop `ext_` 分流；自动门禁全绿，真实浏览器验收待用户确认安装扩展后执行。 |
| 等待人工验收 | [CU-16 · 应用与网站范围的 Host 审批](plan/cu-16-target-approvals.md) | P1 | 在现有 Policy 和显式审批上补目标范围，生产原生操作在本任务落地后才接入。 |
| 已实现 | [CU-17 · GUI 观测查询与协议版本接入](plan/cu-17-gui-observation-protocol.md) | P5 | 为 GUI 提供有界目标状态与截图证据查询，保留 Host 作为唯一入口。 |
| 已实现；自动门禁通过，真窗验收覆盖入口/空态/刷新/断线重连；empty 态与授权回执端到端待 CU-20 目标登记后人工验收 | [CU-18 · GUI 目标选择与权限交互](plan/cu-18-gui-target-approvals.md) | P5 | 让用户选择后台操作目标并管理应用/网站授权。 |
| 已实现；自动门禁通过；真窗验收覆盖证据区空态（no_workspace / no_run）与 observation_list 真实往返；观测行 / 停止状态机待活动 run（环境阻塞，见任务记录） | [CU-19 · GUI 截图证据与停止状态](plan/cu-19-gui-evidence-stop.md) | P5 | 让用户查看实际操作前后证据，并可靠停止后台任务。 |
| 自动回归通过；CU-06/07/08 探针动作验收与 CU-14 滚动复验完成、回放核对零重执行；GUI 模型链路被用户配置 fail-closed 拒绝（行为正确），观测行/停止状态机、CU-15 扩展、并发无干扰与真实接管让出待人工验收 | [CU-20 · macOS 后台能力端到端验收与文档收口](plan/cu-20-native-acceptance.md) | P6 | 闭合首阶段本机后台核心能力，证明用户可持续工作且操作证据完整。 |

### 后续环境与能力

以下是当前对话已规划的后续任务；平台/授权条件尚未满足，不代表已经启动或实现。Windows 同样遵守无干扰目标，用户主桌面的前台输入不作为完成方式。

| 状态 | 任务 | 阶段 | 简介 |
| --- | --- | --- | --- |
| 条件性 | [CU-21 · 隔离 Linux X11 / AT-SPI 应用控制](plan/cu-21-isolated-linux.md) | 后续环境 | 为现有专用 Linux 桌面增加 Rust 应用/窗口和元素控制通道。 |
| 条件性 | [CU-22 · Windows 独立会话 / VM 后台能力](plan/cu-22-windows-isolation.md) | 后续平台 | 通过 Rust Windows 系统 API 实现独立环境中的应用、窗口、UIA、截图和输入。 |
| 条件性 | [CU-23 · Wayland 隔离后台路线可行性](plan/cu-23-wayland-feasibility.md) | 后续平台 | 验证 Wayland 是否存在满足无干扰目标的独立桌面截图与输入路线。 |
| 条件性 | [CU-24 · macOS 锁屏使用独立设计与验证](plan/cu-24-macos-locked-use.md) | 后续能力 | 研究并验证锁屏下的受限后台操作，不把普通 AX 后台操作等同锁屏支持。 |
| 条件性 | [CU-25 · 显式授权的目标音频采集](plan/cu-25-audio.md) | 后续能力 | 补齐目标音频录制或模型使用链路，并明确二者的能力边界。 |

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

| 状态 | 任务 | 简介 |
| --- | --- | --- |
| 等待人工验收 | [RV-01 · 图片附件交互人工验收](plan/rv-01-image-attachments.md) | 验收图片缩略图、预览与移除的分离，以及键盘和草稿保持。 |
| 等待人工验收 | [RV-02 · 文本附件与不可信边界人工验收](plan/rv-02-text-attachments.md) | 验收文本附件的包装、正文折叠和历史重放一致性。 |
| 等待人工验收 | [RV-03 · Composer 附件错误人工验收](plan/rv-03-composer-errors.md) | 验收错误本地化，以及附件类型和大小限制的准确反馈。 |
| 等待人工验收 | [RV-04 · 子代理工具展开人工验收](plan/rv-04-subagent-tools.md) | 验收子代理工具的目标、参数、结果和拒绝状态。 |
| 等待人工验收 | [RV-05 · 子代理回执阅读人工验收](plan/rv-05-subagent-receipts.md) | 验收 Markdown 回执、长回执折叠与无重复全文。 |
| 等待人工验收 | [RV-06 · 子代理标签字号人工验收](plan/rv-06-subagent-font-sizes.md) | 验收 100% / 125% / 150% 字号下标签的真实像素。 |
| 等待人工验收 | [RV-07 · 子代理正文键盘与朗读人工验收](plan/rv-07-subagent-accessibility.md) | 验收正文真正可由键盘访问和朗读。 |
| 等待人工验收 | [RV-11 · 子代理断线与恢复人工验收](plan/rv-11-subagent-reconnect.md) | 验收断线保留已加载对话、重连恢复和诚实加载态。 |
| 等待人工验收 | [GUI2-01 · 工作台壳、首页与侧栏人工验收](plan/accept-gui-workbench.md) | 验收首页、Header 和侧栏在真实任务、项目与连接状态下的可达性。 |
| 等待人工验收 | [GUI2-02 · 对话阅读层级与 Composer 人工验收](plan/gui2-02-conversation-composer.md) | 验收用户与助手消息、长内容阅读和模型管理入口。 |
| 等待人工验收 | [GUI2-03 · 任务与操作快捷查找人工验收](plan/gui2-03-quick-search.md) | 验收当前 Snapshot 范围内的任务、页面和安全导航查找。 |
| 等待人工验收 | [GUI2-04 · 当前对话查找与回合定位人工验收](plan/gui2-04-conversation-navigation.md) | 验收已加载正文查找、长对话回合目录与阅读位置保持。 |
| 等待人工验收 | [GUI2-05 · 工作面板与窄窗可达性人工验收](plan/gui2-05-work-panels.md) | 验收 Inspector 的并排 / 中央呈现及已接通工作面板。 |
| 等待人工验收 | [GUI2-06 · 设置查找与行式信息组织人工验收](plan/gui2-06-settings-search.md) | 验收设置页面 / 行查找、定位、权限控件和供应商信息层级。 |
| 等待人工验收 | [GUI2-07 · 受影响主路径与旧缺口收口验收](plan/gui2-07-main-path-acceptance.md) | 汇总 GUI2 各项验收证据，闭合跨壳层、对话和面板的剩余缺口。 |
| 等待人工验收 | [GUI3-01 · 工具调用可扫读人工验收](plan/gui3-01-tool-summaries.md) | 验收工具 headline、目标、展开内容与真实执行状态。 |
| 等待人工验收 | [GUI3-02 · Composer 元信息与模型入口人工验收](plan/gui3-02-composer-metadata.md) | 验收项目信息、模型 chip、Context 与无项目首页文案。 |
| 等待人工验收 | [GUI3-03 · 代码块头、链接与表格人工验收](plan/gui3-03-markdown-content.md) | 验收 Markdown 表格、代码复制和链接操作。 |
| 等待人工验收 | [GUI3-04 · 失败卡与下一步人工验收](plan/gui3-04-run-failure-next-step.md) | 验收 Run 终态、错误原因、认证入口和 Review changes 条件。 |
| 等待人工验收 | [GUI3-05 · SVG 图标与语义颜色人工验收](plan/gui3-05-svg-icons.md) | 验收图标在真实窗口中的清晰度、语义颜色和操作入口。 |
| 等待人工验收 | [GUI3-06 · 侧栏降噪与字号密度人工验收](plan/gui3-06-task-rail-density.md) | 验收任务分组、侧栏密度和真实 live 状态点。 |
| 等待人工验收 | [GUI3-07 · Header 动作与断线层级人工验收](plan/gui3-07-header-recovery.md) | 验收 Header 导航、状态 chip 与断线恢复入口的层级。 |
| 等待人工验收 | [GUI3-08 · 层次 token 与有限动效人工验收](plan/gui3-08-surfaces-motion.md) | 验收深色层次、交互反馈与 Switch / Inspector 的实际动态效果。 |
| 等待人工验收 | [GUI4 · StatusBar、空态与终端人工验收](plan/gui4-status-empty-terminal.md) | 验收三栏状态、共享空态和终端直接输入的剩余用户路径。 |
| 等待人工验收 | [AC-02 · IME、模型目录与跨面板交互人工验收](plan/accept-ime-model-navigation.md) | 闭合系统 IME、目录操作及字号、错误和焦点的交互验收。 |
| 等待人工验收 | [AC-03 · 产品 Plan 窗口键盘人工验收](plan/accept-plan-keyboard.md) | 闭合 Plan GUI 的完整键盘矩阵；本项指产品窗口，不是 docs/plan 目录。 |
| 等待人工验收 | [AC-04 · 技能录制产物后续 Run 发现验收](plan/accept-skill-reuse.md) | 证明录制技能能被真实后续 Run 作为资源发现。 |
| 等待人工验收 | [AC-05 · Pawork 任务消耗窗口人工验收](plan/accept-task-usage.md) | 闭合 Pawork 统计窗口的用户验收；两个外部消费者拆为独立任务。 |
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
