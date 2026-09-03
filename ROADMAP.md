# Pawork 路线图

> 2026-09-01 重置。本文是当前任务与后续顺序的唯一计划事实源；旧阶段的逐片记录不再保留在活动路线图中，已完成事实查阅 [docs/history.md](docs/history.md)，逐字内容查阅 git 历史。架构红线与冻结契约仍以 [docs/architecture.md](docs/architecture.md) 和源码/golden 为准。

## 1. 当前指针

| 字段 | 当前事实 |
| --- | --- |
| 活动线 | **Settings — 真窗口与人工收口** |
| 状态 | 🟢 SET-1～SET-6g 已实现、通过各自定向门禁并提交；⚠️ SET-7 已完成一轮正式 Host/Desktop 的 Computer Use 真窗口验收：Host 未启动时连接失败→Reconnect、Connected 状态的 Advanced/About、外观 150%→Desktop 重启 100%、默认模型写入与 Composer 同步、移除二次确认与失效默认提示均通过。完整窗口/认证矩阵及人工签字仍未完成，因此 SET-7 未归档。 |
| 当前交付 | [Settings Feature Spec](docs/spec/settings.md)、[Settings 任务书](plan/settings.md)、ADR-046 协议词汇与 Host 门面（verify-then-replace、按 provider 单飞、Global 层默认项写盘）、ADR-047 通用页 wire、ADR-048 权限与审批页 wire、ADR-049 工具与 MCP 页 wire、ADR-050 终端页 wire、SET-6e Desktop 本地外观页、SET-6f Desktop 本地连接诊断页、[ADR-051](docs/adr/ADR-051-about-settings-host-data-dir.md)（Accepted：API 1.9 `host_data_dir`）与 SET-6g About 页。 |
| 下一动作 | 补齐 Kimi Platform/Kimi Code 真实凭证、OAuth device flow/refresh/取消、API key 替换失败保旧、Host 重启与默认项恢复、Connected→Disconnected/Connecting、About 断线/旧 Host 隐藏、1440×1024、125%、Cmd+=/Cmd+-/Cmd+0、Tab/Enter、1080×720 长页滚动与折叠线以下 AX 几何证据；随后由用户完成视觉层级、secure input、OAuth 浏览器切换与 VoiceOver 签字，再归档 Settings 阶段。 |
| 当前阻塞 | Kimi 双通道无真实凭证；本机网络代理不可用，远端模型目录只能确认诚实回落而不能确认远端成功；API key“替换失败保旧”未构造；Computer Use 对 GPUI 滚动容器无效，已用拉高窗口覆盖当前写路径，但 1080×720 长页滚动仍待人工；四项人工签字待用户。 |
| 发布 | **不在本计划内**。待功能继续完善后，由用户另行指定发布范围、License 与门禁。 |

状态：⚪ 未开始 · 🔵 进行中 · 🟢 已验证 · ⚠️ 阻塞。`已实现`、`自动门禁通过`、`真实环境通过`、`人工验收`、`已发布`必须分别记录。

## 2. 目标、范围与完成口径

### 2.1 目标

在 Pawork Desktop 增加真实 Settings 入口和宿主驱动的设置面。第一条纵向主路径是“模型与供应商”：用户可以添加供应商、选择该供应商实际支持的认证方式、验证/移除凭证、刷新可用模型，并设置默认 provider/model。

首批产品范围固定为：

- Z.AI / GLM：API key，先复用当前 Coding Plan 通道；
- Kimi：Kimi Platform API key 与 Kimi Code OAuth，二者端点和凭证语义分开；
- DeepSeek：API key；
- xAI / Grok：OAuth Device Flow 与 API key；当前 OAuth 已有宿主基础，API-key adapter 仍需补齐。

### 2.2 非目标

- 不发布、不做安装器、自更新、签名、公证、供应链或三平台发布矩阵。
- 不在首批实现同 Provider 多账户池、轮询切号、额度路由或团队凭证共享。
- 不在首批开放任意 OpenAI-compatible 自定义端点；保留为后续候选。
- 不让 Desktop 直连 Provider、auth 文件、配置文件或数据库。
- 不为尚无 Host 能力的设置项绘制可点击假页面，不伪造 quota、模型或登录成功状态。
- 不新增包、JS Runtime、第二套配置系统或第二套 Secret 存储。

### 2.3 用户可观察的完成口径

1. TaskRail 底部 `Local` 行有键盘/AX 可达的 Settings 入口；进入后可返回原工作台，active session、草稿和 Run 不被改变。
2. Settings 使用独立左侧导航和完整内容区；模型与供应商页列出 Host 权威连接状态、认证方式、端点语义、模型来源与错误。
3. 添加向导完成“选择供应商 → 选择认证方式 → 登录/录入 key → 验证 → 获取模型”；取消、失败、超时、断线和重试均有诚实终态。
4. API key 只进入 auth backend；不进入 config、command ledger payload、Agent 事件、DB、日志、诊断、fixture 或可提交文件。
5. 可远程获取模型时优先使用已认证的供应商目录；失败时才使用有来源和版本标记的内置目录，并明确显示 `远程 / 内置回退 / 不可用`。
6. 用户选择的默认 provider/model 由 Host 持久化并在重启后恢复；失效选择显式降级，不静默切换供应商。
7. 四家供应商至少各有一条真实认证与模型目录证据；OAuth、API key、Secret 泄漏、断线恢复和模型回退有定向回归。
8. Settings 新增控件具备可见、键盘与 AX 同源 gate；1440×1024 与 1080×720 主操作可达。

## 3. 已锁定的产品规则

- **连接实例而非全局 key 文本框**：首期每个供应商只允许一个活动连接；切换认证方式等价于替换该连接。多账户另行立项。
- **认证能力由 Host 声明**：Desktop 不按供应商品牌硬编码 OAuth/API key 分支；宿主返回可用认证方法、状态和操作能力。
- **认证与模型目录分离**：登录成功不等于目录刷新成功；两个状态和错误分别呈现。
- **目录优先级**：已认证远端目录 > 版本固定的内置目录；远端只负责“当前账号可见 ID”，静态元数据只补显示名/能力/限制，合并时保守且 fail-closed。
- **无公开目录时固定模型**：Z.AI 等未找到稳定公开 list-model contract 的通道使用实现时核对的官方模型页与 [Models.dev](https://models.dev/) 固定快照；不在运行时依赖第三方聚合站作为权限事实源。
- **只展示可运行模型**：图片、视频或其它 Pawork adapter 尚不能调用的模型不进入 Composer 可选列表。
- **Settings 页面诚实启用**：先显示“模型与供应商”；通用、权限与审批、工具与 MCP、终端、外观、高级、关于只在对应真实能力到位时逐页启用。

完整需求、供应商依据与开放决策见 [docs/spec/settings.md](docs/spec/settings.md)。

## 4. 执行顺序

| 阶段 | 状态 | 交付与退出条件 |
| --- | --- | --- |
| SET-0 文档立项 | 🟢 | 重写活动路线图；建立 Feature Spec、任务书和 Settings GUI 规则；旧 plan 不保留。 |
| SET-1 契约与 ADR | 🟢 | ADR-046 Accepted（2026-09-01，初始未发布版本不采取兼容策略）；protocol 新词汇 + registry 登记 + 43 帧 golden + typegen 落地，`cargo test -p pawork-protocol --features typegen --offline --lib --tests` 148 绿。 |
| SET-2 Host 设置门面 | 🟢 | 六入口 handler + descriptor 元数据（`ChannelPreset.display_name`/`auth_methods`）+ `verify_api_key` 验证入口 + Global 层 `write_default_model_pair` + `AuthChanged` 广播落地；四包定向测试全绿。 |
| SET-3 Settings 壳 | 🟢 | gear、Settings Rail、返回工作台、全宽内容区与只读供应商页实现并审查提交（审查修复合计六处；desktop 定向测试 163 绿）；无假按钮，写操作属 SET-4/SET-5。 |
| SET-4 四家认证 | 🟢 | 2026-09-02 完成：registry 扩为八通道（+kimi-platform API key、+kimi-code Device OAuth，端点经 kimi-cli 官方源核对）；auth_methods 改数据字段，xAI 双认证；替换=互斥删旧凭证；Settings 写操作 UI（secure 输入/OAuth 等待/取消/Replace/Remove，AuthChanged 六态消费）。审查修复七处（flight 种类化、Replace 终态重查等）；推送前复查再修 Removed 权威重查 / Verify tooltip / secure 换行三处。Host 471 绿 + Desktop 168 绿；真实账号验收待凭证（SET-7）。 |
| SET-5 模型发现与默认项 | 🟢 | 2026-09-02 完成：xAI/Kimi Code 远端目录（language-models 可运行过滤 / kimi-cli 同端点，SET-D08 关闭）、未知 ID 保守合并、失败一律 fixed_fallback；provider_auth_status 透出持久化默认项且 set 后同会话内存同步；Desktop「模型与默认项」区（分组列表/Default 徽标/Set default/显式刷新，四路径同 gate）、Host 确认后 Composer 同步、失效默认显式提示。审查修复四处（P1 内存同步、P2 空目录误报、P2 kimi 失败路径补测、提交前 HOME Drop 守卫）。providers 163+29 / app 177+23 / desktop 173 全绿；真实 API 与真窗口验收待 SET-7。已审查提交。 |
| SET-6c 工具与 MCP 页 | 🟢 | 2026-09-03 完成：ADR-049 Accepted（`McpTest`/`McpServerRemove` 两命令，since=V1_7 仅 GUI 非幂等，API 1.7；golden 4 帧 + typegen，35 既有帧仅 api_version 机械重写）；workspace `write_mcp_server_remove`（Global RMW+原子写）；app 两 handler（remove 定序含跨层同名守卫与 pawork.mcp.* 清密，同会话生效）；desktop「工具与 MCP」页（Test/Remove 两步确认，回执即权威）。审查（glm_reviewer）修复 1 P1 + 3 P2 + P3。提交前复查再修：写盘成功后清密失败仍同步内存、回执 bump epoch、Settings 页可见 Test/Remove 失败、空态去重、AX 确认不覆盖 last_error、writer 移除/缺失键测试。protocol 152 / workspace 120+13+15 / app 189 / desktop 181 全绿；真窗口验收登记 SET-7。 |
| SET-6b 权限与审批页 | 🟢 | 2026-09-03 完成：ADR-048 Accepted（`PermissionsSettings` 查询含 Host 权威 attached workspace_id、`SetApprovalMode` 严格五值 snake_case 会话内生效不持久化、`WorkspaceTrust` 死词汇实装开放 GUI 会话内信任切换不写盘）；golden 48→55 帧 + typegen + API 1.6；Desktop 权限与审批页（五档选择/会话信任开关/Global 默认只读/生效边界文案/回执才生效）。审查修复 2 P2 + 1 P3（冻结契约回写、状态矛盾、别名越契约、workspace_id 来源错位）；推送前复查再修 scheduler Arc-swap / AX 导航 / 信任开关 id 来源。真窗口验收登记 SET-7。 |
| SET-6a 通用页 | 🟢 | 2026-09-02 完成：ADR-047 Accepted；`GeneralSettings` 查询 + `SetProxyUrl` 命令（必填字段、显式 null 清除、缺字段解码错误）、API 1.5、golden 43→48 帧；`write_proxy_url` Global 原子写；handler 校验预构 client→写盘→内存换入定序；Desktop 通用页（capability gate、生效边界文案、stale 三路径同禁、重连刷新）。审查修复 4 P2 + 3 P3；提交前复查再修导航焦点丢失与 Save 失败文案。protocol 150 / workspace 147 / app 203 / desktop 176 全绿。真窗口验收登记 SET-7。 |
| SET-6 其余 Settings 面 | 🟢 | “权限与审批 → 工具与 MCP → 终端 → 外观 → 高级 → 关于”已逐页完成自动门禁；真窗口与人工验收统一进入 SET-7。每页均以真实能力启用，未接通能力不显示。 |
| SET-6d 终端页 | 🟢 | 2026-09-03 完成：ADR-050 Accepted（Global `[terminal]` shell/columns/rows、非 Global 层整段剥离防仓库投毒、`TerminalSettings` 查询 + `SetTerminalSettings` 全态写、terminal_create 应用配置默认、Desktop 初始尺寸取生效值、API 1.8）；glm_worker 三切片串行 + glm_reviewer 审查修 2 P2（冻结契约回写、Disconnected 补终端页 stale）；提交前复查再修 Save 空 shell→null 与 protocol Spec 模块树计数。protocol 154 / workspace 149 / app 215 / desktop 185 全绿；真窗口验收登记 SET-7。 |
| SET-6e 外观页 | 🟢 | 2026-09-03 完成：Settings Rail 新增始终可用的「外观」页，复用唯一 `TextScale` 与 `set_text_scale` 提供 100%/125%/150% 三档；页面、既有 Cmd+=/Cmd+-/Cmd+0 与 AX Press 同步同一会话状态，当前档有文字/视觉/AX selected 标记；深色主题与 macOS Increase Contrast 仅诚实只读说明。离线 AX 主路径及未知档位 fail-closed 回归通过，desktop 186 全绿；只读复审修复 render/AX 按钮几何漂移与 150% 下长说明裁切 2 P2，并复核 resolved。提交前复查再修：desktop 包级 Spec 模块地图回写 SET-6e（settings/mod/accessibility 行数与测试计数）与外观页 AX `tree.validate` + 150% Press permits。未新增 Host wire、持久化、主题体系或依赖。正式隔离 Host/Desktop 已接通，界面抓取因 Computer Use 超时未取证，视觉/VoiceOver 仍登记 SET-7。 |
| SET-6f 高级页 | 🟢 | 2026-09-03 完成：Settings Rail 在「外观」后新增始终可用的「高级」页；Desktop 保留当前连接已有的非 Secret 握手摘要，展示连接状态、Host runtime ID、协商 GUI API、granted capabilities、启动 endpoint、resume 与最后 ack。Connecting/断线清空协商字段，Failed/Disconnected 复用既有 Reconnect；runtime ID 明确不等同配置 instance，GUI token/token path 不展示，原样 endpoint 不用于反推 data directory/配置 instance。只读复审修 2 P2（Reconnect 入口最终 gate + 握手清空/last-ack 定向证据）。提交前复查：Connection 诊断行改为纯相位文案（Connected / Connecting… / Disconnected / Connect failed），不再复用 TaskRail「Local · Connected · resume」合成串，resume 与 runtime ID 留在各自行。未改 protocol/config/schema/Policy、未新增依赖；Desktop 186/186。真窗口、Tab/Enter 与 VoiceOver 仍登记 SET-7。 |
| SET-6g 关于页 | 🟢 | 2026-09-03 完成：[ADR-051](docs/adr/ADR-051-about-settings-host-data-dir.md) Accepted；API 1.9 Accepted 握手追加可选 `host_data_dir`，GUI Host 单次解析后与 Core/socket/PID/token/握手共用，client/Desktop 原样透传。About 仅在当前连接路径非空时显示 Desktop build、协商 API、Host data directory；缺字段/Connecting/断线隐藏并退回高级页，render/AX 同源，无 updater/release/License。只读审查修复 1 P2（trim 仅判空，不改写合法路径展示）；64 个 GUI fixture 保持不变（44 个当前版本 fixture 1.8→1.9），28 command/15 query 不变；protocol 155、client+CLI 125、desktop 186 全绿。真窗口与人工验收登记 SET-7。 |
| SET-7 真窗口收口 | ⚠️ | 2026-09-04：正式隔离 Host/Desktop 通过 Computer Use 验证 Host 未启动时连接失败→Reconnect、Connected 状态的 Advanced/About、外观 150%→Desktop 重启 100%、DeepSeek 默认项写盘并同步 Composer、Remove 两步确认与失效默认提示；AX/截图仅见脱敏凭证，真实配置已逐字节还原、真实 auth 未改。四家认证/远端目录全矩阵、替换失败保旧、Host 重启与默认恢复、连接完整三态、About 隐藏态、1440×1024、125%/快捷键、Tab/Enter、1080×720 长页/AX 几何及四项人工签字仍待补，未归档。 |

每阶段的写入集、命令和停止条件见 [plan/settings.md](plan/settings.md)。阶段失败先收敛当前层，不自动扩大到下一阶段。

## 5. 开放决策与硬前置

| ID | 决策 | 当前状态 |
| --- | --- | --- |
| SET-D1 | GUI 如何传入 API key，且不进入 command ledger、事件、DB、日志或可重放响应 | ADR-046（Accepted）拍板：`ApiKeySecret` 非重放单帧内存传递，ledger 只缓存响应，响应/事件只携带脱敏元数据。 |
| SET-D2 | Settings 所需 GUI capability/wire 是在现有 Auth command 上补字段/开放可用性，还是追加最小 command/query | ADR-046（Accepted）拍板：新增 `auth_set_api_key` / `auth_cancel` / `set_default_model` 命令与 `provider_auth_status` 查询，并开放 `auth_start` / `auth_remove` 的 GUI 可用性。 |
| SET-D3 | Z.AI 首期只提供 Coding Plan preset，还是同时开放 General API preset | 首期锁定 Coding Plan；General API 后续按真实需求激活。 |
| SET-D4 | Kimi OAuth 的模型目录是否有稳定、公开且可复用的 contract | 实现时以 Kimi Code 官方行为复核；不稳定则用有版本标记的内置目录。 |
| SET-D5 | Settings 本地展示偏好是否需要持久化 | 仅路由/展开态可本地保存；业务默认项必须由 Host 持久化。SET-6e 首片字号明确为当前 Desktop 会话级，重启恢复 100%；不预建通用 preference 框架。 |
| SET-D6 | About 如何获得当前 Host 的权威数据目录 | [ADR-051](docs/adr/ADR-051-about-settings-host-data-dir.md) Accepted：API 1.9 Accepted 握手追加可选 `host_data_dir`；GUI Host 与 Core 共用同一次目录解析，缺字段时 About fail-closed 隐藏。已实现并通过定向门禁。 |

冻结 wire/config/schema、Secret 生命周期或架构边界变化必须先走 ADR；普通 UI 布局与现有查询消费不借机扩张协议。

## 6. 验证与状态回写

- 纯文档任务：相对链接、状态词汇、外部依据、`git diff --check` 和写入集检查；不运行 Cargo。
- 实现任务：默认单个 Cargo 进程运行受影响包的 `cargo test -p <crate> --offline --lib --tests`；该包无测试或只需类型检查时才用 `cargo check -p <crate> --offline`。
- 协议/Secret/持久化改动：对应 golden/typegen、安全种子和泄漏检查不可推迟；真实凭证只从个人 auth backend/环境注入，不写入 fixture。
- Desktop 改动：先定向测试，再用正式 Host/Desktop 和真实状态做窗口验收；截图不替代宿主/磁盘事实。
- 完成一片后同步本文件、任务书、产品 Spec 与涉及包的包级 Spec；已完成过程压缩追加到 [docs/history.md](docs/history.md)。

每次收尾至少记录：

```text
Implemented: <生产路径/用户入口，或 none>
Validated: <实际命令/检查，或 none + 原因>
Targeted regressions: <覆盖，或 none>
Real-world evidence: <环境/账号/窗口，或 pending>
Known gaps: <剩余缺口与登记位置>
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 7. 任务约定

1. 开始前复核源码、当前 diff、相关包 Spec 和真实远程文档；不得按旧计划或记忆改代码。
2. 写清目标、非目标、验收标准和不改动范围；每片控制在数小时内可独立验证。
3. 保留用户未提交改动；只触碰当前切片必需文件，不新增包或生产依赖，除非任务已证明必要并获授权。
4. 改 wire/schema/config/安全语义时先 ADR/golden；用户未 Accepted 前只允许研究和文档起草。
5. 现有测试能证明行为时不新增测试；确需新增时只覆盖本次主路径，必要时再加一个关键失败路径。
6. 不执行全量 workspace 门禁、提交、推送或发布，除非用户另行明确要求。
