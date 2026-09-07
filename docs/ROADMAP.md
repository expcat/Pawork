# Pawork 活动路线图：Desktop 优化（OPT）

> 基线日期：2026-09-05。状态：**OPT-D 六张候选稿已交付、已获视觉签字；OPT-1 已实现并通过定向验证；OPT-2 已实现且真窗口验收通过（§10.3），完成效果审查修复 5 项缺陷并通过定向测试与真窗口复验（§10.10）；OPT-3 内核/协议/配置半区已实现（3a/3b，ADR-055，API 1.12，§10.4），GUI 控件批次（启用弹层/四默认角色/代理 Switch）已实现并经定向门禁与协议层验收（§10.5），代理 Switch 像素级复验已通过（§10.6）；OPT-4 已实现（4a–4e，§10.6）且真窗口对照新图验收通过（§10.7）；OPT-3d/3e 已实现（ADR-056，API 1.13）并真窗口验收通过（§10.8），desktop 门禁 210/210；OPT-3 完成效果审查修复 5 项缺陷并通过定向测试、CLI/Desktop 构建与隔离实例真窗口复验（§10.11，desktop 门禁 212/212）**。来源：当日正式 Desktop 真窗口走查（11 条反馈）。本文件是当前活动线的任务规划，**不是**源码或冻结契约的事实源。P0–P2 收尾证据仍见 [Desktop Spec §8](spec/desktop.md#8-gui-收尾验收记录2026-09-05)；未排期候选仍见 [backlog.md](spec/backlog.md)。

**闸门**：凡涉及显示效果的条目，必须先完成 **OPT-D 统一 UI Design**（一体出图），再改像素与布局。内核/配置/协议可与出图并行准备，但 GUI 落地以设计稿为准。

**配置原则（F5 / F10）**：Settings 里用户能改的项都要写入配置文件，重启后仍在。Host 权威项与代理同一套 Global `config.toml`（macOS：`~/Library/Application Support/dev.pawork.pawork/config.toml`）。Desktop 本地项（语言、字号）同样落盘到用户配置目录。凭证仍只进 auth backend，不进 `config.toml`。仓库内 `<workspace>/.pawork/config.toml` **不得**覆盖代理、信任提升与本次新增的全局偏好。

---

## 1. 反馈总表

| ID | 摘要 | UI Design | 内核 / 协议 / 配置 | GUI 落地 |
| --- | --- | --- | --- | --- |
| F1 | 六处操作图标偏小、不够醒目 | **必须** | — | OPT-4 |
| F2 | Settings 内容列未拉满窗口宽 | **必须** | 现有 820px 内容上限要随设计稿改合同 | OPT-4 |
| F3 | 代理并入供应商配置；同供应商多 OAuth/API key；下拉展开额度 | **必须** | 多凭证是 G1 切片；额度条无真实来源则 fail-closed，不造假（G2） | OPT-3 |
| F4 | Settings 侧栏选中微移；选中态重做；设置页与主界面一体出图 | **必须** | — | OPT-4 |
| F5 | 设置参数像代理一样持久化 | 否（行为） | OPT-1；审批模式持久化须 ADR（现行会话内、不落盘） | OPT-1 |
| F6 | Inspector 默认隐藏 | **必须** | — | OPT-4 |
| F7 | Session 行右侧：改名、归档 | **必须** | 存储有 `archived` 但无公开归档/改名写口；GUI 命令缺失 | OPT-2 |
| F8 | 供应商页顶部：默认对话 / 命名 / 识图 / 搜索模型 | **必须** | 对话默认已有；命名/识图/搜索为新配置与路由 | OPT-3 |
| F9 | New task 直接开无项目任务；浮层位置不对 | **必须** | `SessionCreate` 现必绑 `workspace_id`；Unassigned 仅历史无绑定 | OPT-2 |
| F10 | 每家供应商弹出模型列表，启用/关闭（含全开全关）；下拉隐藏未启用 | **必须** | 配置无 enabled；`ModelList` / Composer / 默认项候选都要过滤 | OPT-3 |
| F11 | 供应商代理改为 Switch | **必须** | 已有 `use_proxy` 与 `set_provider_use_proxy` | OPT-3 |

显示效果（图标、选中态、留白、折叠、按钮、弹层、Switch、展开行、进度条槽）一律算 UI Design，不在实现阶段临时「先凑合画」。

---

## 2. 阶段与依赖

```text
OPT-D 统一出图 ─────────────────────────────────────────┐
        │                                                │
        │ 可并行：OPT-1 持久化（现有控件落盘）              │
        ▼                                                ▼
   设计稿签字                                      OPT-1 完成
        │
        ├─► OPT-4 工作台 + Settings 壳层（纯视觉/布局）
        ├─► OPT-2 会话：无项目新建 / 改名 / 归档 / 自动标题
        └─► OPT-3 供应商与模型：启用集 / 默认角色 / Switch /（后）多凭证
```

顺序纪律：

1. **OPT-D 未签字，不改显示相关实现**（含 icon 尺寸、Inspector 默认、Settings 宽、选中态、New task 浮层、供应商行、模型弹层）。
2. OPT-1 不依赖新像素，可与出图并行。
3. OPT-2 / OPT-3 的协议与配置可在出图期间起草 ADR + golden；GUI 控件等设计稿。
4. F3 多凭证与额度条放到 OPT-3 后段：先启用集与默认角色，再多账户；额度无权威数据源就不画数字。

---

## 3. OPT-D — 统一 UI Design（闸门）

**目标**：主窗口与 Settings **同一套视觉语言一次出图**，覆盖全部显示项。旧 P0–P2 三张图继续作为已交付基线，**不**用来否决本轮需求。

出图清单（至少）：

| 画幅 | 必须交代 |
| --- | --- |
| 工作台 1440×1024，Inspector **收起** | F1 图标尺寸与 hit area；F6 默认隐藏与重新打开入口；F7 改名/归档；F9 无项目 New task（无错误位置的项目浮层） |
| 工作台，Inspector 打开 | 打开后的 Changes/Terminal/Resources；折叠控件足够明显 |
| Composer 模型菜单 | 只出现已启用模型；按供应商分组 |
| Settings 壳 | F2 内容拉满；F4 侧栏选中无位移（背景/内描边，不靠加边框挤文字） |
| Settings → Models & providers | F8 顶部四默认项；F3 可展开凭证区；F11 Switch；F10 模型启用弹层（含全开/全关） |
| 模型启用弹层单独状态 | 空目录、未连接、部分启用、全关后 Composer 为空的诚实空态 |

验收：设计稿检入 `design/`（本轮新文件，不覆盖 P0–P2 三张），更新 [design/README.md](../design/README.md) 与 [gui-design.md](gui-design.md)。截图走查不替代出图。

**任务**（可一份设计交付，不拆成互斥写入集）：

- **OPT-D1** 工作台：图标、Inspector 默认、session 行操作、无项目新建、模型下拉。
- **OPT-D2** Settings 壳：全宽、导航选中、与主界面同一 token。
- **OPT-D3** 供应商页：默认模型区、Switch、展开凭证、模型启用弹层、额度槽（可标「无数据时隐藏」）。

---

## 4. OPT-1 — 设置持久化

对应 F5，并约束后续新设置（F8/F10/F11）同样落盘。

| 任务 | 内容 | 写入集（预计） | 前置 |
| --- | --- | --- | --- |
| OPT-1a | 盘点 Settings 八页：哪些已落 Global `config.toml` / auth.json，哪些仅内存或仅当次窗口 | 文档 | 无 |
| OPT-1b | 审批模式持久化：现行 `set_approval_mode` 只改内存。改 Global 配置须 **ADR**（安全语义），golden 先行 | protocol / workspace / app | ADR |
| OPT-1c | 会话信任 vs `trust_workspaces` 全局项：产品上要「再开还在」，但不得让 workspace 层配置自我提权 | app / workspace | 与 1b 同 ADR 或分 ADR |
| OPT-1d | Appearance：语言、字号写入用户配置，重启恢复；Desktop 不直写 Host 业务键 | desktop + 可选 workspace schema | 无 |

已落盘、本阶段不重做：Network `proxy_url`、供应商 `use_proxy`、默认对话 `default_provider`/`default_model`、Terminal 设置、凭证（auth backend）。

---

## 5. OPT-2 — 会话与无项目任务

对应 F7、F9，以及 F8 的「未命名 session 自动命名」。

| 任务 | 内容 | 内核 | GUI（等 OPT-D） |
| --- | --- | --- | --- |
| OPT-2a | New task / 空态主按钮：**直接**创建不绑 workspace 的会话，归 Unassigned。左栏 `+` 仍可「选项目或添加项目」。All projects 下不再强制 WorkspaceConfirm | `SessionCreate.workspace_id` 改为可选；Host 允许 NULL 归属 | 浮层位置与触发源按设计稿；Composer 旁不再误锚项目菜单 |
| OPT-2b | Session **改名** | 新 GUI 命令 + storage 更新 `title`（现无公开写口） | 行右侧按钮，按设计稿 |
| OPT-2c | Session **归档** | storage 今日「无归档写口、list 隐藏 archived」；补 `archive_session` 与 GUI 命令，不在本阶段做永久删除 | 行右侧按钮；列表默认不显示已归档 |
| OPT-2d | 未命名 session 对话后自动标题 | 新配置「命名模型」（OPT-3b）；Engine/App 在标题仍为占位名时调用该模型，成功才写回。失败保留「New session」，不用启发式冒充模型命名 | 列表即时显示新标题 |

无项目会话的工具/路径闸：无 `workspace_id` 时文件类工具 fail-closed（现有 Policy 已按 workspace 约束）；只适合问答、搜索等不碰仓库的任务。设计稿与文案须诚实。

---

## 6. OPT-3 — 供应商、模型启用与默认角色

对应 F3、F8、F10、F11。

| 任务 | 内容 | 内核 | 说明 |
| --- | --- | --- | --- |
| OPT-3a | 每供应商模型启用集；全开/全关；Composer / `ModelList` / 默认项下拉不出现未启用模型 | `ProviderConfig`/`ModelConfig` 增 enabled（或显式 disabled 列表）；写盘；协议命令；过滤权威在 Host | 关掉当前默认模型必须显式失效，禁止静默换供应商 |
| OPT-3b | Settings 顶部四默认项，候选 = **已连接且已启用** 的模型 | 对话默认已有 `set_default_model`。新增：命名模型、识图模型、搜索模型的配置键与读写 | 识图路由依赖模型 `image_input`（[B5](spec/backlog.md) 未作为内置附件产品面时，只保存选择、带图请求再接线）。搜索路由依赖搜索工具（[B1](spec/backlog.md)）；未落地前只保存选择，不画假搜索 |
| OPT-3c | 代理是否开启改为 Switch，写回已有 `set_provider_use_proxy` | 无需新契约 | 纯 GUI；等 OPT-D |
| OPT-3d | 同供应商多 OAuth / API key，行右下拉展开各凭证状态 | G1 切片：凭证模型从「每 provider 一套」扩到多份；Secret 仍只进 auth backend | **不**一次做完 G1–G6 账户池/亲和路由 |
| OPT-3e | 展开区额度进度条 | 无 QuotaSnapshot 权威来源则 **不渲染数字**，不为填图造假（G2） | 设计稿预留槽位即可 |

F3「代理保存到对应配置」：现行 Global `[[providers]].use_proxy` 已满足落盘；OPT-3c 只改控件形态并与展开后的供应商卡放在一起。

---

## 7. OPT-4 — 工作台与 Settings 壳层落地

对应 F1、F2、F4、F6，以及 F9 的视觉位置。依赖 OPT-D 签字。

| 任务 | 内容 |
| --- | --- |
| OPT-4a | Token / 图标尺寸 / 28px 以外的可见主操作（F1 六处 + 设计稿新增） |
| OPT-4b | Inspector 默认折叠；宽屏不再默认占 440px；打开入口按设计稿加大 |
| OPT-4c | Settings 内容列拉满；取消或重定义 820px 上限（改 [gui-design.md](gui-design.md) 与 AX 几何同源） |
| OPT-4d | Settings 导航选中态：占位稳定，无点击位移 |
| OPT-4e | 空态 New task、左栏 `+`、项目菜单锚点与 F9 行为对齐 |

窄窗 1080–1279 的 Inspector 折叠已在 [BK-RESP-01](spec/backlog.md) 接受延期；本轮 F6 是 **宽屏也默认隐藏**，与 BK-RESP-01 合并验收，不另开第三套布局。

---

## 8. 明确不做（本线）

- 不把 G1–G6 账户池、预算 gate、缓存亲和一次做完；只做 F3 所需的「每供应商多凭证」最小切片。
- 不造假额度、假模型、假搜索结果。
- 不引入 Node/JS；不改四层 Desktop 架构；Desktop 仍只依赖 `pawork-client`。
- 不把全局代理/审批写进仓库 `.pawork/config.toml`。
- 不覆盖 P0–P2 已验收的三张阶段图；本轮新图另存。
- 发布、全量门禁、三平台安装器仍是 [BK-RELEASE-01](spec/backlog.md)，未授权。

---

## 9. 契约与文档同步

实现触及下列项时，**同批**更新对应文档，golden 先于 wire 改动：

- GUI 命令/查询/config schema → ADR + [architecture.md](architecture.md) §3.2 + [contracts.md](spec/contracts.md) + 包级 Spec
- Settings 行为 → [settings.md](spec/settings.md)
- 主窗口信息架构 → [gui-design.md](gui-design.md) + [design/README.md](../design/README.md)
- 用户可见能力 → [capabilities.md](spec/capabilities.md)

建议 ADR 主题：审批模式持久化；`SessionCreate` 可选 workspace；模型启用集；默认角色模型（命名/识图/搜索）；多凭证最小切片（若扩 auth 索引）。

---

## 10. 状态

| 阶段 | 状态 |
| --- | --- |
| OPT-D | 六张统一候选稿已交付、尺寸/状态走查通过；**用户视觉签字通过**（设计闸门已放行） |
| OPT-1 | 1a–1d 已实现；本轮审查修复 3 项缺陷，608 项定向测试、Desktop 构建与单窗口外观保存/重启复验通过（§10.9）；未归档/未发布 |
| OPT-2 | 2a–2d 已实现（ADR-054，API 1.11）；初次验收见 §10.3；本轮审查修复 5 项缺陷，622 项定向测试、CLI/Desktop 构建与真窗口复验通过（§10.10）；未归档/未发布 |
| OPT-3 | 3a/3b 内核·协议·配置已实现（ADR-055，API 1.12）；GUI 控件批次已实现（§10.5），协议层验收通过、修复 D3a 缺陷；代理 Switch 像素级复验已通过（§10.6）；3d/3e 已实现（ADR-056，API 1.13）且真窗口验收通过（§10.8），desktop 门禁 210/210；完成效果审查修复 5 项缺陷，workspace/auth/app/desktop 定向测试、CLI/Desktop 构建与隔离实例真窗口复验通过（§10.11，desktop 门禁 212/212） |
| OPT-4 | 4a–4e 已实现（§10.6）：图标命中区/字形、Inspector 默认折叠 + 重开入口、Settings 全宽与导航零位移、4e 核对一致；desktop 门禁 207/207；真窗口对照新图验收通过（§10.7） |

本线 OPT-1～OPT-4 全部任务已实现并验收；发布与全量门禁仍是 BK-RELEASE-01（未授权），不随本线推定。


### 10.1 本批交付与证据（2026-09-05）

- **OPT-D1/D2/D3**：六张新 1440×1024 PNG 已写入 `design/`，旧 P0–P2 三图保留；[设计索引](../design/README.md#0-opt-d-统一设计交付2026-09-05已签字) 与 [GUI 设计 §8](gui-design.md#8-opt-d-统一候选稿已签字) 同步。出图和技术走查完成，用户视觉签字已于 2026-09-05 确认；本批没有落实 OPT-2/3/4 像素或新控件。
- **OPT-1a**：[八页持久化盘点](spec/settings.md#opt-1a八页持久化盘点) 已完成；代理、默认对话、Terminal 和凭证沿用既有写口。
- **OPT-1b/1c**：[ADR-053](spec/settings.md#adr-053opt-1-设置持久化2026-09-05) 与配置 golden 先行（先红后绿）。审批默认和 canonical 根路径信任只允许 Global 层；写盘成功再更新 Host，进行中 Run 保留快照。逐项目 true/false 优先于全项目信任默认；显式启动参数仅当次覆盖。Run、Terminal 与 Snapshot 按实际目标项目读取信任。
- **OPT-1d**：语言、字号及快捷键共用 Desktop `desktop.json` 保存入口；缺失默认，损坏文件保留并可见报错。正式窗口首帧前恢复，不触碰 Host 业务键。
- **真实窗口复验**：使用本次 `pawork-desktop` 正式二进制，独立验证 bundle/instance、未加载 fixture。离线 Appearance 中选择中文和 125%，确认 `desktop.json` 为 `{"language":"zh","text_scale":125}`，停止并重新启动进程后窗口/AX 同时恢复两项。未建立 Host 连接，不将此次离线外观验证当作 Provider/live Run 验证；结束后还原测试前配置。Host 持久化由 Settings→写盘→`AppCore::load_for_catalog` 重新装配测试证明，生产账号上的权限设置未改动。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-workspace -p pawork-app --offline --lib --tests` | 371 passed |
| `cargo test -p pawork-desktop -p pawork-cli --offline --lib --tests --features gpui/runtime_shaders` | 278 passed（Desktop 191 + CLI/ACP 87） |
| `cargo build -p pawork -p pawork-desktop --bins --offline --features gpui/runtime_shaders` | 通过；复用 target，无新依赖 |
| `bash -n scripts/pawork-desktop.sh` 与启动审批参数分支（空值/显式/非法） | 通过 |
| 设计 PNG 尺寸、文档本地链接、`git diff --check` | 通过 |

定向回归：Global 权威/仓库提权剥离、审批与信任重新装配恢复、显式启动覆盖、项目间隔离、未知模式/未知 workspace 拒绝、写失败保旧、外观保存重读与损坏文件保护。日志位于本机 `/tmp/pawork-opt-{host-tests,desktop-tests,build,config-red}.log`，不检入仓库。安全审查子代理因消息路由错误未能启动；主代理直接复核 Global 剥离、逐项目 Run/Terminal 取值与先写盘后更新顺序，不记为独立模型审查通过。

Validated: 上表实际命令、窗口/AX 与配置文件交叉验证。
Targeted regressions: 上述 OPT-1 核心持久化与权限边界。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.2 本批交付与证据（2026-09-05，OPT-2）

- **ADR-054**（[desktop Spec](spec/desktop.md#adr-054opt-2-会话生命周期与自动标题2026-09-05)）先行冻结契约：GUI API 1.10 → 1.11，golden/typegen 先红后绿。
- **OPT-2a**：`session_create.workspace_id` 可选化（缺省/null → 落盘 NULL 归 Unassigned）；Desktop 全局 New task 直建无项目会话，WorkspaceConfirm 浮层整批移除，项目头「+」与 Add project 保留；无项目会话 Composer 显示 No project chip 与文件工具不可用诚实提示。
- **OPT-2b/2c**：storage `rename_session`/`archive_session`（缺失 fail-closed）；`session_rename`（空白标题结构化拒绝不写盘）/`session_archive`（仅隐藏不删除，wire 保留反归档）GUI 命令；写盘成功回执写后状态并广播 `SessionMetaChanged`；Desktop 会话行右侧 32×32 改名/归档按钮（键盘/AX 可达，行内编辑 Enter/Esc）。
- **OPT-2d**：Global 配置 `naming_provider`/`naming_model`（分层同 default 对，`write_naming_model_pair` 原子写）；GUI RunStart 成功终态后独立 spawn 自动命名（复用/装配命名 provider，无工具一次性补全，20s 超时、限长 72），写回前二次复核占位名，未配置/失败/超时保留 `New session` 不启发式。Settings GUI 入口留 OPT-3b。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-storage -p pawork-protocol -p pawork-app --offline --lib --tests` | 通过（19 个测试二进制，含 storage rename/archive、protocol golden `golden_opt2_session_lifecycle_frames`、registry 31 命令双射钉住） |
| `cargo test -p pawork-workspace -p pawork-app --offline --lib --tests` | 通过（含命名对写读往返、自动标题成功写回/未配置不调用/失败保留占位名三例） |
| `cargo test -p pawork-desktop --offline --tests --features gpui/runtime_shaders` | 193 passed（含 wire 形状钉住、snapshot 改名/归档刷新、行内改名决策、AX 直建改写） |
| `cargo test -p pawork-protocol --features typegen --offline --test typegen` | 通过（schemas 三产物同步检入） |
| `cargo check -p pawork --offline`（主代理收口） | 通过 |
| `git diff --check`、文档本地链接 | 通过 |

定向回归：无归属会话落盘与 snapshot 归组、改名空白拒绝、归档隐藏且 `get_session` 仍可读、SessionMetaChanged 广播与 Desktop 刷新、命名模型未配置/失败诚实保留占位名。未跑 probe/spawn_e2e 与真窗口验收（OPT-2 收尾待做：会话行按钮与行内改名视觉、无项目提示、真实命名模型端到端，用 `opencode-go/glm-5.3-flash` 当次参数）。日志在本机 /tmp（opt2_*/pawork-opt2d-*.log），不检入仓库。

Validated: 上表实际命令。
Targeted regressions: 上述 OPT-2 契约与行为。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.3 本批交付与证据（2026-09-05，OPT-2 真窗口验收 + 无项目问答修复）

- **验收中修复（根因）**：ADR-044 D3 的 `workspace_for_session_or_unbound` 在注册表存在任一可用项目时对未绑定会话一律 fail-closed，与 ADR-054 D1「无项目会话可问答」冲突；桌面 Host 启动即自动登记 cwd 项目，导致无项目会话发送必报 `session … has no workspace binding`（真窗口实测复现）。修复：显式 NULL 归属（合法产品状态）以空授权面 `ws-unbound` 运行问答，文件类工具仍由 Policy 对空 roots fail-closed；绑定悬空维持 fail-closed。[architecture.md](architecture.md) 冻结契约与 [app Spec](spec/crates/app.md) 同批修订。
- **真窗口验收**（macOS 26.6.2，正式脚本构建；隔离实例 `opt2acc`，Host 当次 `--provider opencode-go --model glm-5.3-flash` 覆盖，不写持久默认；生产实例 desktop 全程未受影响）：
  - 2a：全局 New task 直建无项目会话（无 WorkspaceConfirm 浮层），归 Unassigned；DB `workspace_id` 为 NULL；Composer 显示 No project 与文件工具不可用诚实提示；真实问答 Run 三次 completed。
  - 2b：会话行 Rename → 行内编辑，Enter 提交（DB 写后状态）/Esc 取消（不落盘）。
  - 2c：Archive → 列表即时隐藏，DB `archived=1`，事件与投影未删除。
  - 2d：Global 临时写入 `naming_provider`/`naming_model`（opencode-go/glm-5.3-flash，验收后已还原），Run 成功终态后自动命名为模型生成标题，SessionMetaChanged 广播使窗口列表即时更新。
  - 附带复验：Host 重启后窗口 Reconnect 恢复连接、会话与 Composer 草稿。
- **启动发现**：运行中的 `Pawork.app` 被脚本 `cp -f` 覆盖二进制后，新进程从同一 bundle 路径启动即被 SIGKILL（exit 137，无日志）；改用平行 bundle 路径规避。已记入 [AGENTS.md](../AGENTS.md) §11 工程经验。
- **自动验证**：`cargo test -p pawork-app --offline --lib --tests` 225 passed（含 unassigned→unbound 主路径与 dangling 绑定 fail-closed 两条新回归）；`cargo build -p pawork --offline` 通过（复用 target，无新依赖）。

Validated: 上表实际命令 + 真窗口/AX/SQLite 交叉验证。
Targeted regressions: 无归属会话问答、空授权面 fail-closed、改名/归档写读、自动标题写回。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.4 本批交付与证据（2026-09-05，OPT-3a/3b 内核·协议·配置）

- **ADR-055**（[settings Spec](spec/settings.md#adr-055opt-3-模型启用集与默认角色2026-09-05)）先行冻结契约：GUI API 1.11 → 1.12，golden/typegen 先红后绿；`disabled_models` denylist 与 vision/search 角色键 Global 层独占（非 Global 层剥离 + 告警）。
- **OPT-3a 内核**：`SetModelEnabled` / `SetProviderModelsEnabled`（全关按当前聚合目录展开，目录为空 `catalog_unavailable` 不写盘）；禁用命中角色默认对时同批清除并回执 `cleared_roles`（固定序 conversation→naming→vision→search，禁止静默换绑）；`ModelList` 缺省过滤禁用模型（`include_disabled=true` 取全量），RunStart / switch_provider / set_default* 对禁用模型一律 `model_disabled` fail-closed。
- **OPT-3b 内核**：Global 新增 `vision_provider/vision_model`、`search_provider/search_model`（naming 对已在 ADR-054 落地）；`SetDefaultRoleModel{role, value}`（未知 role fail-closed，null 清除）；`provider_auth_status` 响应增 `role_defaults{naming,vision,search}`。Vision/Search 落地期只保存选择、不接路由（B1/B5 未落地）。
- **不在本批**：OPT-3 GUI 控件（模型启用弹层、四默认角色区、代理 Switch，对照 OPT-D 签字稿）、OPT-3d/3e（多凭证、额度槽）、真窗口验收与发布。
- **Desktop 消费面**：`provider_auth_status.role_defaults` 自 1.12 起必填；Desktop 夹具 / AX 与 `pawork-client` 再导出已同步，`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 193/193。GUI 控件仍待后续批次。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-protocol -p pawork-workspace -p pawork-app --offline --lib --tests`（单 Cargo 进程收口） | 通过（protocol 165 + workspace lib 132 / loader_file 13 / smoke 15 + app lib 207 及 gui_server 集成全绿） |
| protocol golden 先红后绿：升版首跑全红，`GUI_PROTOCOL_UPDATE_GOLDEN=1` 重生成后转绿；既有 fixture 逐 token 核对仅 minor 11→12 + `role_defaults` 新增键 | 通过 |
| `cargo run -p pawork-protocol --features typegen --bin pawork-protocol-typegen` + `--check` | 通过（schemas 三产物同步检入，零漂移） |
| `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` | 193 passed（`role_defaults` 必填后夹具/client 同步） |

定向回归：三命令 golden（含 clear）与 model_list 两态 fixture、registry 双射、required-nullable 缺键 fail-closed、schema denylist parse/merge、writer set/clear/保留未知字段、loader 两类剥离告警、启停 roundtrip、cleared_roles、全关展开/空目录 fail-closed、角色设/清/未知 role、model_list 两态、RunStart 双路径 model_disabled。实现由 glm 子代理按互不重叠写入集串行切片（protocol → workspace → app），主代理撰写 ADR-055 与 Spec/ROADMAP 同步并抽查切片 diff。

Validated: 上表实际命令。
Targeted regressions: 上述 OPT-3a/3b 契约与行为。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.5 本批交付与证据（2026-09-06，OPT-3 GUI 控件批次）

- **范围**（对照 OPT-D 签字稿）：每 provider「Manage models」启用弹层（单模型 Switch + Enable all / Disable all + 空目录诚实空态）、页首「Default models」四默认角色区（候选 = 已连接且已启用模型，可清除，vision/search 标注只保存不接路由）、代理开关改 Switch 控件（OPT-3c，写回仍走 `set_provider_use_proxy`）、Composer 模型选择器过滤禁用模型并在全禁用时显示「No enabled models」诚实空态且禁用发送。
- **实现方式**：glm 子代理三片并行（Composer / 四角色区 / 供应商弹层与 Switch），写入集互不重叠；主代理收口审查、协议层验收与文档同步。
- **验收中发现并修复一处数据丢失缺陷（ADR-055 增 D3a）**：禁用流程原按内存生效配置判定角色默认对清除，而 CLI `--provider/--model` 覆盖只进内存不落盘——带覆盖运行的 Host 上 Disable-all 会把覆盖值误判为用户默认对，误删盘上真实的 `default_provider/default_model`（本机实际发生一次，xai/grok-4 被删，当场从验收前备份恢复）。修复：清除判定改读盘上持久化配置（Builtin + Global 文件，不含 Session/Run 覆盖），内存同步仅在与被清持久化对一致时执行，CLI 覆盖的生效值保留。回归：`disable_keeps_persisted_role_pairs_under_memory_override`。
- **协议层复验**（真实运行测试 Host `--instance opt3acc --provider opencode-go --model glm-5.3-flash`）：盘上 xai/grok-4 默认对 + 内存 CLI 覆盖场景下，Enable-all → `cleared_roles=[]`；设 naming=opencode-go/deepseek-v4-pro 后 Disable-all → `cleared_roles=["naming"]`，盘上默认对完好、naming 键对被移除、内存覆盖值保留；代理开关 on/off 往返落盘一致。探针教训：复用 command_id 会命中持久幂等账本重放（响应真实但不执行写入），复验脚本必须每次换新 command_id。
- **真窗口核对**：四角色菜单分组完整性（含 MenuPanel 240px 折叠滚动后 xai/glm-coding 组可达）、Composer 选择器、Manage models 弹层已在真窗口经 AX + 截图核对；代理 Switch 的像素级开关操作待屏幕解锁后补验（交互三路径已由 AX 门禁钉住）。两处疑似缺陷结案为非缺陷：角色菜单「缺组」是弹层折叠滚动假象；角色触发器「索引点击无反应」是误点相邻静态文本标签，AX 派发代码无误。
- **配置影响披露**：验收向共享 Global 配置写入过 6 条 `use_proxy=false` 与 opencode-go denylist（验收操作本身即写该文件），收尾已恢复为验收前 87 字节备份（仅 `default_*` 与 `proxy_url`）；测试 Host/Desktop 为独立 `opt3acc` 实例与平行 bundle，未触碰生产实例（pid 68191）。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` | 205 passed（含弹层/角色区/Switch/空态 AX 钉板与 Composer 空态 fail-closed） |
| `cargo test -p pawork-app --offline --lib --tests` | 208 passed（含 D3a 回归）+ gui_server 集成 21 全绿 |
| 协议层复验（Enable/Disable-all、cleared_roles、盘上默认对保留、代理开关落盘往返） | 通过 |
| 真窗口像素级代理 Switch 操作 | 待屏幕解锁补验 |

Validated: 上表实际命令与协议探针；真窗口核对范围如上。
Targeted regressions: D3a 内存覆盖不误删盘上默认对（单禁 + 全关两路径）；弹层/角色区/Switch/空态 AX。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.6 本批交付与证据（2026-09-06，OPT-4 工作台与 Settings 壳层落地）

- **范围**（对照 OPT-D 签字稿）：4a 六处主要动作命中区 ≥36×36、可见字形 20px（`font::ICON`）——rail 分组切换/全局与项目头「+」/Local gear、Workspace Header Activity 与 header-new-task、Inspector 面板内 collapse、Composer Send/Cancel 同槽（Composer 常态自然高 89，仍在 88–94 合同）；4b Inspector 初始默认折叠（仅构造默认，Review changes / Activity 摘要等显式动作仍可展开），折叠态 Header 最右新增 `inspector-expand` 重开按钮（40×37 槽、与 Activity 触发器 4px 间距并存，click/Enter/Space/AX Press 同 handler，render/AX 经 `HEADER_ACTION_GAP` 同源）；4c Settings 内容取消 820px 上限，用满 Rail 外可用宽度、两侧各 32px padding（`SETTINGS_CONTENT_PAD`，render 与 AX 同源，含 7 个 AX 页文件 50 处原点机械迁移）；4d 导航选中/未选中共用同一外壳几何（微移根因：选中态 mt_2 差 8px + gpui border 参与 Taffy 布局致焦点描边挤内容盒），差异仅为背景/字重/绝对定位左缘 3px 指示条，1px 描边两态常驻、焦点只换色，文字坐标零位移；4e 空态/左栏 +/菜单锚点对照签字稿核对一致，无改动。
- **实现方式**：glm 子代理两片并行（工作台 / Settings 壳），写入集互不重叠且不跑 cargo；主代理收口编译与门禁、修复一处遗漏钉板（mod.rs Composer 动作槽 32→36）、撰写文档同步。
- **未采纳的签字稿视觉**（维持现行生产合同，改动须先改 gui-design.md，见 §8）：空态装饰性气泡图标、rail 全宽蓝色 New task 按钮。`⤢` 字形已在真窗口确认渲染正常（见下段顺带观察）。
- **配置影响**：OPT-4 实现本身纯 GUI，无配置/协议/契约变化（ADR-055 不变，API 1.12 不变）；Global 配置仅在下段 Switch 复验中被写测并已还原，生产实例全程未触碰。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（单 Cargo 进程，主代理收口） | 207 passed（较 §10.5 记录 +2：OPT-4d 导航零位移 AX 主路径断言 1 条 + §10.5 批次计数复核差 1 条）；OPT-4a token 钉板（36 命中区/HEADER_ACTION_GAP）、Composer 高度合同重钉 89∈[88,94]、Inspector 默认折叠断言（复用既有 AX 宿主）全绿 |
| `git diff --check`、文档本地链接 | 通过 |

定向回归：六处主操作命中区/字形 token 钉板与 AX 同源（rail/header/composer/collapse）、Inspector 默认折叠与重开入口三路径、Settings 全宽列 render/AX 同源（含 50 处 AX 原点迁移）、导航两态 frame 逐像素一致。OPT-4 真窗口对照新图验收仍待补做（本段仅记录顺带观察，不替代正式验收）；OPT-3 代理 Switch 像素级复验同批已通过（见下段）。日志在本机 /tmp（pawork-opt4-desktop-tests*.log），不检入仓库。

**同批补做：OPT-3 代理 Switch 像素级复验**（屏幕已解锁；隔离实例 `opt4acc` + 平行 bundle，Host 当次 `--provider opencode-go --model glm-5.3-flash` 覆盖；生产实例与 Global 配置验收前已备份、验收后已还原）。xAI 行代理 Switch：第一次点击（元素级）On→Off，盘上 Global config 同批落 `use_proxy = false`；第二次真实像素坐标点击（屏幕坐标 (5368,778)，AX 框中心同源）Off→On，盘上落回 `use_proxy = true`；AX 值随回执翻转（不乐观更新），真实鼠标点击后焦点落在 Switch 上。顺带真窗口观察（不构成 OPT-4 正式对照验收）：Inspector 默认折叠且 Header 右上 Activity + 重开按钮并存、`⤢` 字形渲染正常、Settings 内容全宽、导航选中行左缘指示条无位移。

进程经验：exec 会话内 `nohup … &` 派生的 Desktop/Host 进程在会话收尾时被回收（空日志静默退出），导致 CUA 按路径重拉起无参数实例；GUI 进程须用 `open -na <bundle> --args …`（launchd 托管跨会话存活）或常驻 exec 会话承载。已记入 AGENTS.md §11。

Validated: 上表实际命令 + 真窗口 AX / 像素点击 / 盘上配置交叉验证。
Targeted regressions: 上述 OPT-4 几何与行为；代理 Switch 像素级开关写读往返。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.7 本批交付与证据（2026-09-06，OPT-4 真窗口对照新图验收）

- **环境**：macOS 真窗口，隔离实例 `opt4v` + 平行 bundle `target/pawork-desktop-runtime/Pawork-opt4v.app`（`open -na` launchd 托管），Host 常驻 exec 会话当次 `--instance opt4v --provider opencode-go --model glm-5.3-flash` 覆盖（不写持久默认）；生产实例（pid 68191）与运行中的 opt3acc 旧实例全程未触碰；验收前备份 Global config（87 字节）、验收后 diff 确认零写入（本批纯视觉验收，无配置写操作）；二进制为 HEAD（2ba21cd）无改动增量构建（1.84s no-op 证明新鲜）。
- **4b Inspector**：默认折叠（AX 树无 Inspector 容器）；Header 最右 Activity（inspector-toggle）+ Open inspector（inspector-expand，⤢ 字形渲染正常）并存；点击展开后出现 Changes/Terminal/Resources 三 tab 与面板内 Hide inspector 折叠控件、Changes 诚实空态「No active session」；折叠后回到双按钮并存态。对照 `opt-workbench-inspector-collapsed/open-v1.png` 结构一致（签字稿空态装饰性气泡图标按 §10.6 决议不采纳）。
- **4a 图标**：rail 分组切换（Timeline↔Projects 真实切换验证）、左栏 +、Settings 齿轮、header-new-task、inspector-expand/collapse 全部经真实点击/AX Press 生效；命中区与 20px 字形几何由 desktop 门禁 token 钉板覆盖（§10.6，207/207），真窗口抽查功能与渲染正常。
- **4c Settings 全宽**：Models & providers 与 Network 两页内容列均用满 rail 外宽度（卡片与 Save/Clear 抵右缘），对照 `opt-settings-shell-default-roles-v1.png` 一致。
- **4d 导航选中零位移**：选中态左缘 3px 指示条 + 背景高亮，选中/未选中文字基线 x 坐标截图对比一致（点击产生的焦点描边只换色不挤布局）。
- **4e 锚点与 F9 行为**：空态 New task 与左栏 + 均直建无项目会话归 Unassigned（无 WorkspaceConfirm 浮层），Composer 显示 No project 与文件工具不可用诚实提示；会话行右侧 Rename/Archive 按钮在选中行可见可用。
- **收尾**：kill Desktop 与 Host 进程，无残留 opt4v 进程；Global config 还原校验通过（本无写入）；测试会话留存于实例私有 `~/.pawork/opt4v/session.db`，不影响生产实例数据。

Validated: 真窗口 AX 树 + 截图逐条对照六张签字稿中与本批相关的四张；Global config 验收前后 diff。
Targeted regressions: 本批纯验收无代码改动；几何/行为钉板沿用 §10.6 desktop 门禁 207/207。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.8 本批交付与证据（2026-09-06，OPT-3d/3e 多凭证共存与额度槽）

- **ADR-056**（[settings Spec](spec/settings.md#adr-056opt-3d3e-同供应商多凭证最小切片与额度槽诚实空态2026-09-06)）先行：API 1.12 → 1.13（additive），golden/typegen 先红后绿。范围：同 provider 的 API key 与 OAuth 凭证共存（SET-4 A3 替换语义缩窄为同 kind 覆盖）、`provider_auth_status` Entry 增 `credentials` 逐条状态、provider 卡展开区（Proxy/Manage models/Credentials/Usage 四区）、Usage 恒「Usage unavailable」诚实空态。同 kind 多账户与账户选择/路由不做（G1  backlog）。
- **实现方式**：glm 子代理三片串行（protocol → app+cli → desktop，互不重叠写入集、各自跑本层门禁）；主代理撰写 ADR 与全部 Spec 同步、补 `pawork-client` 的 `ProviderCredentialStatus` 再导出、收口全量门禁与真窗口验收。
- **验收中发现并修复 gpui 文本截断缺陷（两轮）**：展开区行标题/副标题在真窗口渲染为「…」（AX 与几何正常，仅像素截断）。根因：region 根 `min_w_0` 在 taffy 0.9 MinContent 测量趟以 Definite(0) 下探，gpui 0.2.2 nowrap 文本首测量即按 0 宽截断并整帧缓存毒化；flex_row+flex_1 文本列（确定 flex-basis）可躲过内容测量趟。第一轮（worker）修 Proxy/Manage models/Usage 行与 chevron 字形（12px 极小 → 20px `font::ICON` + 36×36 槽）；第二轮 Credentials 区头部仍复现，主代理按同机制把 header 文本列改为 flex_row+flex_1 骨架后真窗口转绿。经验记入 AGENTS.md §11。
- **真窗口验收**（隔离实例 `opt4v` + 平行 bundle，Host 当次 `--provider opencode-go --model glm-5.3-flash` 覆盖；Global config 验收前备份、验收后 diff 零写入；生产实例未触碰）：八张 provider 卡默认折叠、chevron 清晰可见；xAI 展开卡四区齐全——Proxy 行标题/副标题/Switch、Manage models 行、Credentials 区（OAuth `eyJ…c1QQ · Expired`，过期状态诚实呈现）、Usage 槽「Usage unavailable」无数字；GLM Coding 展开卡 API key `ba0…fHL9 · Connected`；Replace/Remove 动作在 Credentials 区下方保留。
- **配置影响**：本批验收纯只读（展开/查看），Global config 零写入（diff 实证）；测试会话留存于实例私有 `~/.pawork/opt4v/session.db`。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-protocol --offline --lib --tests`（protocol 切片） | 通过（golden 12/12；缺键 fail-closed、双 kind roundtrip） |
| `cargo run -p pawork-protocol --features typegen --bin pawork-protocol-typegen` + `--check` | 通过（schemas 零漂移，仅 versions.d.ts 增 1.13） |
| `cargo test -p pawork-app -p pawork-cli --offline --lib --tests`（app 切片 + 主代理收口复跑） | 通过（app lib 213/213、gui_server 集成 6/6+15/15、cli 44/44、acp 全绿；含共存 roundtrip、credentials 固定序/expired/env 不入列、auth_status 双行） |
| `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（desktop 切片 + 截断修复后复跑） | 210 passed（含展开卡 AX 钉板：chevron、凭证行、Usage 恒无数字、proxy gate、空凭证诚实空态） |
| `cargo check -p pawork --offline`（主代理收口） | 通过 |
| 真窗口展开卡对照签字稿（上段） | 通过 |

定向回归：共存 roundtrip（互删语义移除）、credentials 固定序与缺键 fail-closed、auth_status 双行、展开态状态机（显式 ∨ 流程态）、截断修复后 AX 钉板。`git diff --check` 与文档本地链接通过。日志在本机 /tmp，不检入仓库。

Validated: 上表实际命令 + 真窗口 AX/截图对照签字稿 + Global config diff。
Targeted regressions: 上述 ADR-056 契约与行为。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.9 OPT-1 完成效果审查与修复（2026-09-06）

对照 §4 的 1a–1d 和 ADR-053，核对当前实现、八页盘点与实际测试；发现并修复三项缺陷：

- **项目信任隔离（P1）**：工具 Run/Terminal 已按目标项目判断，但 AGENTS/Skills 指令注入仍读取 attached 项目信任。改用目标 workspace 的 canonical roots，防止受信任的 attached 项目放行其他不受信任项目的指令，也避免反向漏载。`session_instructions_follow_target_workspace_trust` 用两个真实临时项目、AGENTS 与有效 Skill manifest，覆盖两种相反信任状态及 Global true 下显式 false 优先；旧实现先红，修复后绿。
- **非 Global 权限键剥离（P2）**：文件先做 schema 校验，仓库层非法 `approval_mode` / `workspace_trust` 会在应被忽略前阻断启动。改为先按层剥离，再逐文件校验；Global 非法值与其他 schema 错误仍带来源路径拒绝。`file_permissions_are_stripped_before_schema_validation` 覆盖 Profile/Workspace/Session/Run 文件层、告警与 Global 拒绝；旧实现先红，修复后绿。
- **外观旧快照覆盖（P2）**：两个窗口依次修改不同外观项，后写窗口会连同缓存的另一项一起覆盖磁盘。保存改为读取当前磁盘值、仅更新用户操作的字段；扩充既有保存重读测试验证语言/字号互不覆盖，未知键及损坏文件保护保留。本次不声称跨进程同时写入互斥。

实现只改 4 个源码文件，无新增依赖、协议或 schema 变化；ADR-053 与 app/workspace/desktop 包级 Spec 同批同步。只读收口审查通过。

真窗口使用隔离 HOME 与平行 bundle（`--instance opt1-review-a`），不连接 Host。窗口保留英文/100% 旧快照时，外部将隔离 `desktop.json` 改为中文/100%，再在窗口选择 125%；磁盘结果为中文/125%，没有覆盖语言。窗口切换中文立即生效；终止旧进程并启动新进程后，AX 与截图确认中文/125% 恢复，实际 argv 与磁盘文件交叉核对。用户原 `desktop.json` / `config.toml` 验收前后哈希一致。两窗口直接切换因原生焦点回调阻塞未完成；两份旧快照顺序保存由自动回归覆盖，实机仅声称上述单窗口与外部更新验证通过，不作为新的人工视觉签字。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-workspace -p pawork-app --offline --lib --tests` | 398 passed（app 214 + 集成 23；workspace 133 + 集成 28） |
| `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` | 210 passed |
| `cargo build -p pawork-desktop --offline --bins --features gpui/runtime_shaders` | 通过 |
| 外观旧快照保存、语言立即生效与重启恢复 | 单窗口真机复验通过（方法与限制见上段） |
| `git diff --check`、变更文档本地链接 | 通过 |

Validated: 上表实际命令、真窗口 AX/截图、隔离偏好文件与进程 argv；用户原配置哈希不变。
Targeted regressions: 目标项目信任隔离、非 Global 权限键先剥离后校验、外观单项保存保留另一项。
Full workspace gate: NOT RUN（当前未设置全量门禁）。


### 10.10 OPT-2 完成效果审查与修复（2026-09-06）

对照 §5 的 2a–2d 和 ADR-054，核对 Host、storage、Desktop 与现有回归；发现并修复五项缺陷：

- **命名误决议审批（P1）**：读取首条用户消息使用了会封闭 pending approval 的恢复入口，延迟执行的命名任务可能拒绝下一回合审批并追加事件。改用 `resume_messages_keep_pending` 只读重放；回归比较完整事件账本与待审批状态，旧实现先红、修复后绿。
- **自动标题覆盖手动改名（P2）**：读标题后再 UPDATE 存在竞争窗口。storage 增 `rename_session_if_title`，以单条条件 UPDATE 校验占位名并写回；不匹配时标题与更新时间均不变，多个结果只允许首个匹配者写入。
- **命名请求阻塞配置操作（P2）**：命名网络请求全程持有 Core 读锁，模型切换等写操作需等待。改为同步快照依赖后释放锁，装配、目录解析与补全共用 20s 上限；返回时复核命名配置仍有效，手动改名或清除角色后旧结果不落盘、不广播。暂停 Provider 的回归实证网络等待期间可取得 Core 写锁。
- **新建后打开错误会话（P2）**：Desktop 忽略创建回执、猜测刷新列表首项，在并发更新时可能选错。改为只使用成功 Data 回执中的非空 `session_id`；缺键、畸形或失败回执报错，snapshot 只刷新列表。
- **归档后草稿与终端项目错位（P1）**：当前会话从 snapshot 消失时，AppView 未同步 Composer 草稿和 Terminal workspace。改为保存原会话草稿、恢复无会话草稿、复位分页与 Changes，并切换 Terminal 的 workspace 与输入草稿；连续 snapshot 仍按当前 UI scope 选终端。真实 AppView 回归覆盖原会话项目与归档后 scope 不同、重复刷新两次的情况。

实现改动限于 6 个源码文件，无新增依赖、schema 或 wire 变化；ADR-054、GUI 设计及 app/storage/desktop 包级 Spec 同批同步。自动验证后只读收口审查通过。

真窗口使用隔离 HOME、数据目录、凭证目录与平行 bundle（`--instance opt2-review`），Host 当次指定 `--provider opencode-go --model glm-5.3-flash`，不写持久默认。验证结果：

- All projects 的 New task 在 SQLite 落为 NULL workspace，界面归 Unassigned 并显示 No project / 文件工具不可用；行内改名 Enter 提交后 Header 与数据库一致，归档后列表隐藏且数据库 `archived=1`。
- 两个真实临时项目各建一个 PTY；项目 B 会话激活时分别填入 Composer 与 Terminal 草稿。归档 B 后恢复无会话 Composer 草稿、项目 A 的 Terminal 草稿与 PTY；实际发送恢复的命令成功，再写入标记文件，确认文件只在项目 A 出现、项目 B 未误写。经协议反归档并重新打开 B 后，两份 B 草稿恢复。
- 结论由窗口截图、AX 状态、Host snapshot、SQLite 与实际文件交叉验证。此轮隔离环境无凭证，未新跑真实模型问答或自动命名；命名边界由上述自动回归验证，既有 live 验收记录见 §10.3。用户原 `config.toml` 哈希及 `desktop.json` 不存在状态前后不变；收尾已关闭两个 PTY 与测试 Desktop/Host，无残留测试进程。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p pawork-storage -p pawork-app --offline --lib --tests` | 410 passed，1 ignored（既有 golden 写入器） |
| `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` | 212 passed |
| `cargo build -p pawork -p pawork-desktop --offline --bins --features gpui/runtime_shaders` | 通过 |
| 无项目创建、改名、归档、草稿恢复与跨项目 PTY 路由 | 真窗口复验通过（范围见上段） |
| `git diff --check`、变更文档本地链接 | 通过 |

Validated: 上表实际命令、真窗口 AX/截图、Host snapshot、SQLite 与项目内标记文件；日志及截图留本机 /tmp，不检入仓库。
Targeted regressions: 命名不改审批/事件账本、原子标题写回、命名释放 Core 锁与丢弃失效结果、创建回执定位、归档后草稿与 Terminal scope 一致。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 10.11 OPT-3 完成效果审查与修复（2026-09-06）

对照 §6 的 3a–3e、ADR-055/056 与当前源码，完成效果审查发现并修复以下缺口：

- **模型启用集与默认项一致性**：单项保存改读最新磁盘值，保留其他实例已保存的禁用项；全关合并已有 denylist，避免暂时离开目录的模型恢复后被重新启用。禁用集与命中角色清除合并为一次原子写盘，失败全部保旧；同一 Host 的写盘与内存更新持同一 Core 写锁，设置角色默认值在写锁内复核启用状态。覆盖旧快照顺序写入，不声称跨进程同时写入互斥。
- **跨供应商同名模型**：聚合目录原先仅按 model id 去重，会丢失另一供应商的同名条目；改按 provider + model 保留，模型弹层与默认角色候选均使用修正后的目录。
- **默认角色空态**：无已连接/已启用候选时保留 Clear，渲染、键盘与 AX 一致；目录成功加载为空时明确判定默认模型失效，保留原选择，不静默切换。
- **代理与凭证实际生效**：API key 验证、OAuth device start/token exchange/refresh 统一遵循供应商代理选择；凭证更新/移除或供应商代理设置变化后，下一轮即使 provider/model 相同也重新装配，避免复用旧凭证或代理。缺凭证启动的目录模式也可在连接后重装配。
- **OAuth 替换与移除竞争**：新登录未返回 refresh_token 时原子删除旧账号 refresh，同时保留共存 API key；普通刷新省略 refresh_token 时仍保留当前账号 refresh。AuthRemove 与同 provider 认证写入共用单飞闸，进行中的认证返回 busy，避免删除后被写回。

3d 仍为同 provider 的 API key + OAuth 共存、同 kind 覆盖、整组 Remove；同 kind 多账户不在本切片。3e 保持 Usage unavailable，不展示无权威来源的额度数字；vision/search 仍只保存选择。无新增生产依赖、schema 或 wire 变化，相关包级 Spec 与 Settings/GUI 设计同批同步。

当前状态：修复已实现；workspace/auth/app/desktop 定向测试、CLI/Desktop 构建与隔离实例真窗口复验全部通过。本次未归档、未发布。

真窗口复验（隔离实例 `opt3-review-live` + 平行 bundle，独立 HOME/数据/auth 目录，本地 mock `/models`；用户真实 config/auth 验收前后哈希一致、未污染）：未连接供应商显示默认模型失效告警；命名角色菜单空候选时 Clear 与空态说明并存，清除后落盘移除 naming 对、行显示 Not set；GLM 展开区 Proxy/Manage models/Credentials/Usage unavailable 齐备，代理 Switch Off 落盘 `use_proxy = false`；经 GUI 连接隔离 key 后 Manage models 可用，Disable all 原子写入 `disabled_models` 并同批清空命中的 default 对，AX 同步为 Off / Not set；Composer 选择器不再出现 GLM 已禁模型，当前标签保留旧选择不被静默改写（fail-closed 设计）。日志与 AX 转储留本机 /tmp（pawork-opt3-review-*），不检入仓库。

Validated: `cargo test -p pawork-workspace --offline --lib --tests`（162 passed）；`cargo test -p pawork-auth -p pawork-app --offline --lib --tests`（314 passed，1 ignored）；adapter 失效状态调整后 `cargo test -p pawork-app --offline --lib`（216 passed）；`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（212 passed）；`cargo build -p pawork -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（通过）；`git diff --check`（通过）；隔离实例真窗口 AX 复验（上段）。
Targeted regressions: 磁盘旧快照/暂退目录禁用项保留、禁用与角色清除原子写失败保旧、同名模型分供应商保留、空候选 Clear、加载为空时默认失效、认证代理路由、旧 refresh 不跨账号继承、认证期间 Remove 拒绝、同模型下一轮不复用旧凭证。
Full workspace gate: NOT RUN（当前未设置全量门禁）。


### 10.12 OPT-4 完成效果审查与修复（2026-09-07）

对照 §7 的 4a–4e、OPT-D 签字稿和当前源码，审查发现三项实现缺口：

- **Settings 导航大字号坐标**：AX 仍按固定 8px padding/gap 推算，实际 `Panel p_2/gap_2` 随 rem 缩放；改为同源 rem 间距并计入右侧 1px 分隔线。原选中态零位移测试改为比较真实 render bounds，覆盖 100%/125%/150% 与选中切换。
- **Appearance 控件坐标**：字号与语言按钮遗漏 16→32px 原点迁移，纵向估算也未跟随实际排版；改读 GPUI 实测框并裁剪至内容滚动视口，复用外观 AX 行为测试覆盖。
- **项目菜单滚动**：原 AX 固定发布 200px 菜单框及全部选项，超过真实 240px 菜单视口后仍给出屏外可执行坐标，键盘高亮也不会滚入视口。菜单与 AX 共用滚动句柄、实测外框和行框；打开时滚入当前项，方向键移动时滚入高亮项，屏外选项不发布。新增一个多项目菜单主路径回归。

本批只修改 Desktop 与相关文档，无生产依赖、配置或协议变化。包级 Spec 中仍写「OPT-4 首次真窗口验收待补做」的过时状态同步更正为 §10.7；本轮修复复验独立记录，不沿用历史验收充当本次结果。

当前状态：代码修复已实现，定向测试、修复后的真窗口复验与增量代码审查均已通过；未归档、未发布。

Validated: `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（213/213）；`cargo build -p pawork-desktop --offline --features gpui/runtime_shaders`；`git diff --check`；隔离实例 `opt4-review`（平行 bundle，binary sha256 `3a84002b…211a`，Host 当次 `--provider opencode-go --model glm-5.3-flash`）真窗口复验通过：Inspector 默认折叠且重开/收起可用；Scope 菜单只发布视口内项目，键盘高亮滚至 Add project，选择末尾 Project-09 后重开自动滚入当前项；Settings 内容全宽，Appearance 按钮 150% 可点击并即时生效，恢复 100%；修复前后 AX/截图证据在本机 `/tmp/pawork-opt4-review/screens`，用户 Global 配置指纹验收前后一致。
Targeted regressions: Settings 三档字号导航实际几何、外观按钮位置/行为、Scope 多项目滚动与键盘高亮；增量代码审查确认 GPUI render 期 `cx.notify()` 抑制问题已改用 `defer_in`，无残留阻塞项。
Full workspace gate: NOT RUN（当前未设置全量门禁）。
