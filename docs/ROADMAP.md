# Pawork 活动路线图：Desktop 优化（OPT）

> 基线日期：2026-09-05。状态：**OPT-D 六张候选稿已交付、已获视觉签字；OPT-1 已实现并通过定向验证；OPT-2/3/4 未开始**。来源：当日正式 Desktop 真窗口走查（11 条反馈）。本文件是当前活动线的任务规划，**不是**源码或冻结契约的事实源。P0–P2 收尾证据仍见 [Desktop Spec §8](spec/desktop.md#8-gui-收尾验收记录2026-09-05)；未排期候选仍见 [backlog.md](spec/backlog.md)。

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
| OPT-1 | 1a–1d 已实现；定向自动验证通过；Appearance 真窗口重启恢复通过；未归档/未发布 |
| OPT-2 | 未开始（协议可预研，GUI 等 D） |
| OPT-3 | 未开始（协议可预研，GUI 等 D） |
| OPT-4 | 未开始（等 D 签字） |

本线整体仍未完成：OPT-2/3/4 尚未开始；设计签字、后续 GUI 对照新图验收与发布分别记录，不由本批自动推定。


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
