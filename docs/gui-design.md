# Pawork Desktop GUI 设计

> Desktop GUI 的设计事实源。P0–P2 收尾记录见 [Desktop Spec](spec/desktop.md#8-gui-收尾验收记录2026-09-05)；当前活动线见 [ROADMAP.md](ROADMAP.md)（GUI 用户体验收口）；上一条 Desktop 模块重设计线（UI-1～UI-6，每个任务重做一个模块的 UI 与交互，OPT-D 旧稿不再否决新视觉）全文见 [历史记录](review/roadmap-ui-2026-09-09.md)。视觉实施基准见 [../design/README.md](../design/README.md)。产品/验收汇总见 [spec/desktop.md](spec/desktop.md)；包级 Spec 见 [spec/crates/desktop.md](spec/crates/desktop.md)。

---

## 1. 目标与非目标

**目标**：本机 Agent 工作台，真实驱动 `pawork`。同一窗口增量加面，不改四层架构。

**非目标**：

- 不嵌入 Core，不直连 Provider / SQLite / 工具 / Keychain。
- 不做 TUI，不做 WebView / JS 壳。
- 不实现插件市场、Hooks 管理、WASM 安装器。
- 不做完整多窗口远程桌面、签名安装器、主题生态。

独立 GPUI 进程，只经 GUI Connection Protocol 连接 CLI。关闭窗口不取消已进入 Core 的 Run。

---

## 2. 参照与取舍

只吸收可验证的「主对话壳」行为，不复制完整 IDE。以 [三张阶段设计图](../design/README.md) 检查信息架构与视觉语言，以真窗口和真实数据判断功能。

| 参照 | 吸收 | 不吸收 |
| --- | --- | --- |
| Codex Desktop | 项目内组织 thread、桌面与 CLI 会话连续 | 多 Agent command center、Worktree 编排、Cloud / Remote |
| OpenCode | 会话继续、当前会话模型切换、工具详情与 permission 可见 | TUI 键位、WebView/JS 插件面板、并行多会话工作站 |
| DeepSeek Harness Web UI | Trajectory / 仅追加会话回放、工具与审批留在同一对话 | Cordis/JS 插件、Web-first 默认壳、Code Mode 编辑器 |
| Cursor Agent / MCP | 工具请求、参数与结果在对话内可展开；需要时就地审批 | 编辑器分屏、代码导航、IDE Settings |
| Zed Agent Panel | Thread 按项目分组、Changes 摘要、模型与 usage 靠近 Composer | 完整编辑器面板、worktree 管理 |

产品形状：**一个本地 Coding Agent 聊天窗**，不是工作站。

---

### UI-3 时间线更新（2026-09-08，进行中）

沿用 UI-1 中性色板。用户反馈指出旧版时间线偏在左侧、宽窗大片空白、消息像密集日志；本次改为居中阅读列，以正文、用户消息和执行细节的差异建立层次。当前已实现的规格如下，覆盖下文历史阶段中冲突的 Timeline 数值：

| 内容 | 当前呈现与交互 |
| --- | --- |
| 阅读列 | 在 Workspace 中居中，最大 880px，窄窗两侧至少各留 28px；底部留 32px。虚拟列表条目用内部 padding 计入消息间距，避免外 margin 未计高造成挤在一起。 |
| 作者与正文 | 作者和时间 12px secondary，与菜单共用一行；正文 16px / 26px 行高，使用整列宽度。作者到正文 12px，段间 12px，消息间 32px。用户消息使用 20px 内边距、12px 圆角浅底卡片；助手回复保持开放排版。 |
| Markdown | 段落、1–6 级标题（同字号加粗）、无序/有序列表、引用、fenced code、粗体、斜体、行内代码；UX-02 补齐常用管线表格，表头加粗、行分隔线、等宽列，支持左右 / 居中对齐、中文、escaped pipe 和行内代码。代码保留原始换行，块内提供复制按钮；HTTP(S) 链接显示完整地址，旁边提供编号的打开 / 复制动作。正文菜单提供完整 Markdown 复制及对应代码 / 链接动作，↑/↓ 与 Enter 可操作，Esc 回焦；AX 使用实际菜单布局。流式未闭合标记保留内容；仍是最小子集，不承诺完整 CommonMark。 |
| 工具组 | 默认收起，36px 无底色摘要汇总真实数量和状态；点击、Enter / Space、AX 共用展开状态。展开后工具名和状态占独立行，完整输出在下面换行，取消单行截断。去掉工具首字母图标，展开区以 1px 左边线与行间分隔线组织。工具组前间距 16px。 |
| 思考 | ADR-057 接通历史与 live；默认收起为 36px「思考 / Thinking」摘要，行前留 16px。展开后显示 14px secondary 全文，四边 12px 内边距；点击、Enter / Space、AX 共用状态，展开保持视口。只显示可见思考，无耗时来源则不画时长；收起时 AX 不含正文。 |
| Run | 同 Run 的旧相位被后继状态吸收；原始事件不删。成功且没有可审阅文件时只显示完成页脚与 Fork 菜单，不重复画空摘要卡，独立页脚前留 12px；失败/取消保留原因卡，有真实 Changes 时保留 Review。UX-02 允许从助手回复菜单定位同 Run 的后继闭合边界分叉；用户消息 / 未完成回合及断线时显示禁用原因，不把消息事件作为分叉边界。 |

**实现边界**：思考投影与默认折叠已按 [ADR-057](spec/desktop.md#adr-057ui-3-思考投影与会话身份2026-09-08) 实现；历史与 live 共用 reducer，committed 全文替换增量并保留思考首次出现的位置，分页与重放不重复、不跨工具移位。只展示可见 thinking，redacted 与 opaque reasoning 不进入 GUI。实现、自动检查、代理真窗口检查和用户人工验收分别见 [路线图](review/roadmap-ui-2026-09-09.md)。

### UI-5 设置更新（2026-09-08）

沿用 UI-1 中性深色 token，设置内容用满侧栏外宽度，两侧各留 32px；非供应商页首尾在 100% 字号下各留 32px，纵向留白与分区间距随字号缩放。参照本节列出的 Codex / OpenCode 设置导航与分区方式，把当前值、编辑控件和生效说明分开组织，保持原有八页与真实能力 gate。

| 区域 | 当前呈现与交互 |
| --- | --- |
| Settings Rail | 40px 导航行、14px 文字、6px 圆角；选中用中性 raised 底与 medium 字重，焦点用共享覆盖描边。返回为低强调按钮，选中与焦点切换不移动文字。 |
| 页头与分区 | 20px 标题、14px 次级文字；页头刷新靠右。分区间距 24px，1px subtle 分隔线后留 24px，区内 12px；说明自然换行。 |
| 控件 | 非供应商页按钮 36px 高、6px 圆角、14px 字；Network / Terminal 的 Save 为 Primary，输入沿用 TextInput，失效态保持原写 gate。 |
| Network / Terminal | 当前值位于输入上方；Terminal 将 Shell 与列数 / 行数分区，尺寸输入有独立可见标签，统一 Save 提交既有三个字段。生效说明单独成区。 |
| Approvals | 五档整行选择保留两行说明与当前状态，下面独立呈现项目信任与 Global 只读默认。没有新增权限模式或写入口。 |
| Tools / Appearance | MCP 空态与错误使用清楚的状态区；服务器卡内留 16px，保留 Test 和两步 Remove。外观分为深色主题、字号与预览、语言；100% / 125% / 150% 与双语选择仍本地即时生效并持久化。 |
| Advanced / About | 标签与值按两列对齐，以细分隔线分行；长路径和值自然换行。Advanced 断线仍能进入并重连，About 仍以认证握手提供数据目录为可用条件。 |
| 无障碍与滚动 | 七个非供应商页用 ScrollHandle 记录真实元素框，prepaint 后同步 AX；只发布滚动视口中的可见部分，离屏节点不发布动作。切页清除上一页测量，字号与语言按钮沿用既有实测框。 |

UI-5 不改变供应商卡、目录、多账号、Host 设置写入或 wire。实现、自动检查和人工验收状态分别见 [路线图 UI-5](review/roadmap-ui-2026-09-09.md#ui-5-本批证据2026-09-08)。

### UI-4 输入栏更新（2026-09-08）

Composer 沿用 UI-1 色板与圆角，与 UI-3 的 880px 阅读列居中对齐。设计参考 [Zed Agent Panel](https://zed.dev/docs/ai/agent-panel#changing-models) 将模型选择放在消息编辑器附近、[上下文用量](https://zed.dev/docs/ai/agent-panel#token-usage-and-compaction) 靠近输入区的组织方式；下列尺寸与分层是 Pawork 的设计取舍。

- 输入卡片两侧至少 28px，顶部 16px，底部元信息到状态栏保留 24px。卡片内边距 16px，输入与动作行相隔 12px，沿用 raised surface、1px 边框与 12px 圆角；聚焦草稿时增强边框，不改变布局。
- 卡片内只保留草稿、模型和发送 / 取消。模型选择使用低强调按钮与下拉提示，220px 固定槽，完整 provider / id 保留在 tooltip 和菜单中；模型和发送均为 36px 高命中区。菜单继续向上打开，键盘、鼠标和 AX 共用既有选择路径。
- 卡片下方显示只读项目 chip，右侧是真实 ContextMeter，缺容量仍显示 unavailable。UX-03 将「文件工具不可用」紧跟无项目状态，并与「在项目中新建任务…」入口同排；普通宽度保持一行，窄窗 / 大字号必要时换行，元信息高至少 28px 并随字号增长，AX 使用实际布局。有项目任务移除限制与入口并回收高度；瞬态反馈另行显示。
- 常态卡片至少 110px，草稿按可用宽度自然换行，显式换行与软换行一起决定高度；向上增长到 220px 后内部滚动。完整字符、光标、选择与 IME 共用换行结果，软换行不改原文；卡片外的留白与元信息不占这份增长预算。占位缩短为「给 Pawork 发消息…」，发送 tooltip 提示 Enter / Shift+Enter。草稿按会话保存、IME composing 阻止发送、空白输入禁用发送、运行中同槽取消、断线保留草稿与全禁用模型空态均沿用既有行为。

UX-03 项目引导：侧栏触发器标明「筛选 · 项目名」，切换不改变当前任务或草稿；隐藏当前任务时明确提示。空态与无项目任务中的「在项目中新建任务…」展开项目列表，选择项目后创建并打开新任务；「添加项目…」打开系统目录选择器，取消无项目 / 会话副作用，确认后等待 Host 返回 canonical 项目再创建。无项目旧任务始终保留，不能通过筛选或新建入口静默重绑。键盘 ↑/↓、Enter 与 AX 共用菜单动作，Esc 返回入口。

实现、自动检查、真窗口与用户人工视觉验收分别记录在 [路线图 UI-4](review/roadmap-ui-2026-09-09.md#ui-4-本批证据2026-09-08)。

### UI-1 工作台视觉更新（2026-09-07）

本线用 [UI-1 规格示意](../design/ui1-workbench-tokens.svg) 记录共享 token；下文旧阶段尺寸与本节冲突时，以本节及源码为准。参照 [Zed Agent Panel](https://zed.dev/docs/ai/agent-panel) 的对话 / 工具面板分层，本批采用中性炭灰背景、紧凑工具栏和低强调页签；这是 Pawork 的设计取舍。

- 色板：canvas `#1e1e21`、panel `#18181b`、menu `#252528`、raised `#26262a`、hover `#303035`、pressed `#222225`；正文 `#f2f2f3`、次要文字 `#aaaab2`、辅助文字 `#9b9ba4`，蓝色主动作 / 焦点继续 `#2f6fed`。次要文字在所有允许背景上对比度 ≥4.5。
- 字阶：Header 18px、面板页签 14px、元信息 12px，继续随 100% / 125% / 150% rem 缩放。共享圆角为 6 / 8 / 12px，间距继续 4 / 8 / 12 / 16 / 24 / 32px，focus ring 2px；普通面板无阴影，仅浮层使用既有 elevation。
- Header 总高 80px，顶部留 24px，标题与元信息间距 12px；动作维持 40×37px 命中区，改用无边框 Ghost、6px 圆角。元信息来自原有权威投影；缺字段隐藏。
- Inspector 宽度仍为 440px，顶层 / 二级页签条 48 / 40px；选中背景与居中 24×2px 中性下划线不推动文字位置。hover / pressed / focus 即时反馈。
- Inspector 开合为 180ms cubic ease-out 宽度过渡，快速反向从当前宽度续接；每帧使 Timeline 测高失效，空间不足立即归零（100% 至少 1288px，150% 至少 1320px，中央始终 ≥560px），resize 不写用户偏好。默认收起、显式重开、cmd-i 和 Changes / Terminal / Resources 原入口保留。
- 状态栏高 30px，将真实状态文案分成四组元信息居中呈现；任务额度 —、未知 token 与吞吐继续如实展示。

本批实现、自动验证、代理真窗口检查与用户人工视觉验收分别记录在 [ROADMAP 历史记录](review/roadmap-ui-2026-09-09.md)；2026-09-07 用户确认 UI-1 视觉通过。UI-2～UI-6 的验收状态同文记录。

## 3. 信息架构

```text
┌──────────────────┬──────────────────────────────────────┐
│ TaskRail         │  Timeline                            │
│  · 分组角标      │   user / assistant / tool / error    │
│  · 项目范围      │                                      │
│  · 连接 / +      │                                      │
│  · 日期 / 项目   │                                      │
│  · Task / +      │                                      │
│                  ├──────────────────────────────────────┤
│ Workspace        │  Composer                            │
│  · 路径          │   输入 · 发送 · 取消 · 模型          │
│  · 连接          │                                      │
└──────────────────┴──────────────────────────────────────┘
```

| Surface | 范围 | 不做 |
| --- | --- | --- |
| Connection / Shell | 发现或拉起本机 `pawork gui serve`、连接状态、断线提示 | 多 instance、远程 Host、updater |
| TaskRail / Sessions | 列表 / 新建 / 打开 / resume；按时间或项目组织 | Fork / 分支树产品页 |
| Timeline | user、assistant 流式、tool 调用起止、错误 | citation、Artifact 分页、thinking 专门产品页（有事件就只读展示） |
| Composer | 纯文本发送、取消当轮、下拉已配置 model/provider | 附件；`@file` 由 Host 在 run_start 展开 |
| Approval | 时间线内嵌仅本次允许 / 本轮运行允许 / 拒绝 | 完整 Policy 说明页、信任向导 |
| Changes / Terminal / Resources | Host-driven Surface | 写能力按各自契约；Terminal 是纯文本视图，不是 VT emulator |
| Settings | 独立 Settings Rail + 全宽内容 | 无真实读写能力的页不显示；不画 updater/License 占位 |
| Workflow | 隐藏 | 真实产品面另行设计 |

空态：已连接且无会话时主区只有一句提示和 Composer。不以假卡片冒充未实现能力。

### 3.1 主路径与状态

启动 Desktop → 连接本机 Host → 恢复 Snapshot 与当前会话历史 → 新建或打开会话 → 发送一轮 → 查看流式文本 / 工具活动 → 必要时审批或取消 → 在终态继续下一轮。一个窗口同一时刻只操作一个 active session。

| 状态 | 必须显示 | 可用动作 |
| --- | --- | --- |
| 首次连接 / 无可用 Host | 正在连接；失败后显示可重试的原因 | 重试或退出；不显示业务假数据 |
| 已连接、当前会话空闲 | 权威 Timeline、当前 model/provider | 发送、新建/打开会话、切换模型 |
| 当前 Run 运行中 | assistant 流式；tool `pending/running/succeeded/failed/cancelled` | 取消当轮；发送与模型切换禁用 |
| 等待审批 | 内嵌工具名、目标、风险与短摘要 | `仅本次允许` / `本轮运行允许` / `拒绝`；仍可取消当轮；无默认允许 |
| Run 已完成 / 失败 / 取消 | 终态留在 Timeline | 继续下一轮；失败信息可复制 |
| 重连中 | 保留内存 projection 但整体标为 stale / 只读 | 禁用发送、模型切换与审批；不得把本地 pending 当权威结果 |
| 协议不兼容 | 明确显示版本不兼容 | 只允许退出/重试；不得降级走 `--json` |

UX-04：连接失败 / 断线在主区显示可读原因、重试与连接诊断；无任务时替代新任务空态，有任务时保留现有 Timeline 和草稿。重试期间显示正在重连，立即再次失败也显示连接尝试次数。诊断进入 Advanced；模型入口同步区分未连接、连接中 / 重连中、失败、目录加载中与无模型。

模型选择器只列 Host 返回的已配置条目；切换只影响下一轮，并以 Core 的确认事件覆盖本地 pending。审批按钮映射 `ApproveOnce / ApproveForRun / Deny`；关闭审批卡片不能等价于允许。

UX-05：Composer 模型菜单打开后将当前项滚入视口，按名称 / ID 不区分大小写筛选；搜索和当前高亮项的连接 / 来源固定在顶部，供应商分组也标注来源。备用目录明确写明不代表认证成功，未知 / stale 状态不冒称已连接；不隐藏 Host 返回的未连接供应商模型。完整名称与 ID 可阅读，鼠标悬停与方向键共用高亮；无结果可清除搜索，Esc 回到触发器。搜索 Return 不发送 Composer 草稿。

### 3.2 TaskRail

阶段视觉基准为 [P0 Foundation](../design/desktop-ui-p0-foundation-v4.png)、[P1 Run & Review](../design/desktop-ui-p1-run-review-v4.png) 与 [P2 Settings & Polish](../design/desktop-ui-p2-settings-v4.png)。

- 顶部 `All projects / <project>` 是范围筛选；`Timeline / Projects` 是分组方式。两者正交。
- 分组方式使用标题行右侧二态直接切换按钮，不保留宽幅 segmented control，也不打开下拉菜单。OPT-4a（F1）起六处主要动作（分组切换、项目/任务新增、Activity、Inspector 折叠/重开、Send/Cancel）命中区 ≥36×36px、可见字形 20–22px；Session 行改名/归档命中区 ≥32×32px 维持不变。
- 当前为 Timeline 时显示 folder icon、tooltip / AX name 为 `Show projects`；当前为 Projects 时显示 clock icon、tooltip / AX name 为 `Show timeline`。图标表达目标动作，AX value 表达当前视图。
- click、Enter、Space 与 AX Press 立即切到另一种分组；切换后焦点留在按钮，active session、Composer 草稿、Run、scope 与项目展开状态不变。
- 全局 `AddTaskButton` 在连接行；每个项目头另有定向新建，绑定该项目 canonical `workspace_id`。断线与 stale 时两类入口均禁用。
- ADR-054 / OPT-2：`All projects` 作用域下全局 New task 直接创建无项目会话（归 Unassigned），不再弹 WorkspaceConfirm 项目确认浮层；项目作用域与项目头「+」仍定向新建。
- ADR-054 / OPT-2：Session 行右侧提供改名 / 归档图标按钮（命中区 ≥32×32，键盘与 AX 可达）；改名为行内编辑（Enter 提交、Esc 取消、空白不提交），归档立即生效（仅隐藏、不删除）。改名/归档/自动标题写回经 `SessionMetaChanged` 推送后重取 snapshot。
- OPT-2 审查修复：创建后打开 Host 回执点名的会话；当前会话归档时保存其 Composer 草稿并恢复无会话草稿，清理旧分页与 Changes，同时切换 Terminal 的 workspace 与草稿；重复 snapshot 不把旧项目输入带入其他终端。
- ADR-054 / OPT-2：无项目会话激活时 Composer 底栏显示 No project 状态 chip 与「文件工具不可用」诚实提示；该 chip 纯展示，不提供挂载项目写口。
- Timeline 层级：日期（Today / Yesterday / Previous 7 days / Earlier）→ 项目 → Task，均按最近活动倒序；Task 行不再重复项目名。
- Projects 按 canonical Workspace 分组；缺失元数据的 Session 进入 `Unassigned`。
- 切换分组不改变 active session、Composer 草稿、Run 或主 Timeline。
- 分组方式、范围筛选和项目展开状态是本地 presentation preference，不新增 domain 事件，也不改协议。

#### UI-2 会话行更新（2026-09-07）

交互参照 [Zed Threads Sidebar](https://zed.dev/docs/ai/parallel-agents) 的悬停归档入口；Pawork 同时提供改名，并保留现有 Host 写口。规格示意见 [UI-2 状态图](../design/ui2-taskrail-states.svg)，这是本线新增规格，不是产品截图。

- 当前会话、指针悬停行、键盘聚焦的行或其动作都立即显示改名 / 归档；键盘聚焦与当前会话独立，不必先打开会话。
- 44px 行高、14px 标题、12px 相对时间；时间与动作共用固定 64px 尾槽，两个按钮各 32×32px，显示切换不挤动标题。长标题保持单行截断。
- 项目头的名称、计数共用完整悬停与焦点面，覆盖到会话行同一右缘；有项目「+」时仍保留独立新建按钮。
- 当前会话使用中性选中面，悬停即时反馈，键盘焦点描边独立显示；沿用 UI-1 色板和 6px 控件圆角。归档用单色 16px 盒形图标。
- 改名编辑期间隐藏行操作，Enter 提交、Esc 取消；断线禁写，归档仅隐藏。可见控件与 AX 共享显示规则，动作点击不打开该会话。
- 范围筛选、分组、项目折叠与定向新建保留既有行为。实现、自动检查与人工视觉验收状态分别见 [路线图](review/roadmap-ui-2026-09-09.md)。

### 3.3 Context、运行信息与 Inspector

- Composer 以本页 UI-4 规格为准：居中卡片至少 110px、最高 220px，模型 / 发送 36px 命中区，项目与上下文位于卡片下方。
- `ContextMeter`：当前请求上下文估算 / model catalog context window。容量未知时显示 unavailable，不用 Session 累计 token 冒充。
- Workspace 与 Inspector 底部共享 30 px `RunStatusBar`：UX-07 显示当前 / 最近 Run 的输入、输出 tokens，仅取持久化终态累计用量；缺值显示 `本轮用量 —`。运行中附加时长，完成后不常驻 quota / tok/s / idle 占位。每个 Run 页脚保留本轮用量；取消仅用页脚，失败原因仍在卡片。工具展开显示已有参数和最终结果，空目录明确显示路径与 0 项，缺结果不能冒充空结果。
- Inspector 顶层：Changes / Terminal / Resources。OPT-4b（F6）起默认折叠（宽屏同样），折叠时宽度归零；Workspace Header 右上 `Activity` 触发器与最右 `inspector-expand` 重开按钮并存（OPT-D 签字稿 collapsed 态），`ActivityPopover` 只摘要已有的 Changes 事实；显式动作（重开入口、Activity 摘要、Review changes）展开面板，空间不足（窄窗 / 大字号）保持折叠。Surface 未接通时隐藏对应分区，不做可点击假入口。
- 只消费 projection / Host capability，经 controller → `pawork-client`；GUI 不直连 Provider、quota、Git、PTY 或数据库。

### 3.4 可见层级

- Timeline 消息、tool 与 summary 占满可用宽度，以 618px 可读列封顶。同一 Run 的连续 tool 合并为一个 group，标题汇总数量与真实状态，默认展开并可由 click、Enter、Space、AX Press 折叠；折叠键取首个 tool event id，live 与 replay 结构一致。
- Run 终态只保留一个 summary；只有当前 Session 存在至少一个真实、可审阅的 Changes 文件时才显示 `Ready for review` 与 `Review changes`，并打开、聚焦 Changes；文件列表为空时显示轻量 `Run completed`，不画假 CTA。失败、取消与审批状态均使用文字 / 图标，不只依赖颜色。
- TaskRail 项目计数使用 56px 右对齐 meta 槽，UI-2 任务时间与动作共用 64px 尾槽；标题 `truncate`。
- Changes 文件行使用固定槽；DiffView 有只读路径 header 与增删 marker gutter；Changes / Resources 的 empty、error、stale 各自给出诚实说明。
- ActivityPopover 内容宽 320px，100%/125%/150% 下内容高 144/180/216px；含 padding/border 的外框为 338×162/198/234px，右缘对齐触发器并保持 8px 下方间距，AX 几何同源；来源说明按实际内容追加。

### 3.5 Settings

Settings 沿用深色主题、8px 节奏和 1440×1024 基线，不把工作台改造成 Dashboard：

```text
┌──────────────────┬────────────────────────────────────────────┐
│ Settings Rail    │  内容区                                    │
│ ← Back to workspace│                                           │
│ Models & providers│                                            │
│ Network           │                                            │
│ Approvals         │                                            │
│ Tools & MCP       │                                            │
│ Terminal          │                                            │
│ Appearance        │                                            │
│ Advanced          │                                            │
│ About             │                                            │
└──────────────────┴────────────────────────────────────────────┘
```

- 入口位于 TaskRail 底部 `Local` 行右侧 gear。进入后左栏换成 Settings Rail；Timeline、Composer、Inspector 不渲染。OPT-4d（F4）起导航选中态零位移：选中与未选中共用同一外壳几何，UI-5 起差异仅为中性背景与字重（焦点描边用绝对定位覆盖层，不参与布局），文字坐标逐像素不变。
- `← Back to workspace` 恢复进入前的 session、Timeline 位置、Composer 草稿、Inspector 和 Run；Settings 不取消 Run。各页内容在受限高度内纵向滚动，切页回到顶部；输入框至少容纳当前字号的一行与内边距。
- Settings 导航 AX 随 `Panel` 的 rem padding/gap 同步缩放，并扣除侧栏分隔线；UI-5 七个非供应商页均使用 GPUI 实测框，只发布滚动视口中的可见部分。项目 Scope 菜单同样使用实测框与滚动偏移，超过 240px 时只发布可见选项；键盘高亮与打开菜单时的当前选项滚入视口。
- 导航与页内可见文案默认 English，可在 Appearance 页切换为简体中文（即时生效，保存到用户目录 `desktop.json`，重启恢复）；顺序为 Models & providers → Network → Approvals → Tools & MCP → Terminal → Appearance → Advanced → About。没有真实读写能力的页不显示；Advanced 离线仍可进入。
- 翻译边界：只翻译界面 chrome 文案（按钮、提示、空态、状态提示、tooltip）；session 标题、provider / model id、文件路径、工具输出与 wire 错误原因等数据内容保持原文；品牌名「Pawork」、功能符号与示例数据不翻译。render 与 AX 经同一目录同源取词，AX 节点 id 保持英文。
- **Models & providers**：OPT-4c（F2）起内容用满 Rail 外可用宽度、两侧各 32px padding，不再保留 820px 上限（render 与 AX 几何经 `SETTINGS_CONTENT_PAD` 同源）；UI-6a provider 使用自然增高卡片，分组显示名称 / 认证方式与连接状态 / 目录模型数；认证操作放在独立详情行，避免窄窗与大字号挤压信息列。Host `provider_auth_status` 是权威数据，Desktop 不按供应商名称硬编码 OAuth/API key 分支。普通行与 AX summary 不显示 masked credential、endpoint、catalog error 或 raw model id；endpoint / 错误只在连接、等待或删除确认详情出现。API key editor 仅在 Connect / Replace 后展开，secure input 的完整值不得进 AX tree、日志或状态文本。OAuth 等待区提供「打开授权链接」「复制链接」及存在 device code 时的「复制验证码」按钮；打开交给系统默认浏览器，仅接受 HTTP(S)，断线 / stale 禁止打开。登录详情（URL、验证码、到期、端点和错误）使用可选中复制的只读字段，长网址横向滚动；支持鼠标拖选、键盘全选 / 复制，按钮支持鼠标、Enter / Space 与 AX Press，复制后显示「已复制」。无 device code 的 PKCE 流程不画复制验证码按钮；终态收起过期动作，API key secure 输入仍只发布掩码。交互参照 [OpenCode 登录界面](https://github.com/anomalyco/opencode/blob/dev/packages/app/src/components/dialog-connect-provider.tsx) 的授权链接与 readOnly / copyable 字段，不接触 token。认证成功与目录成功是两个状态。UX-06 起供应商按已连接优先稳定排序，「Default models」四默认角色区放在供应商列表后（对话/命名/识图/搜索；候选 = 已连接且已启用的模型；无候选时仍保留 Clear，可清除失效默认项；识图/搜索的说明及选择入口直接标注「尚未生效」，仍允许保存偏好）。每 provider 行提供 Manage models 弹层：固定头部展示名称 / ID 搜索、连接与目录来源、完整目录启用计数和批量动作；520px 高度上限内由模型列表独立滚动，长名与 ID 可横向阅读，↑/↓ 和 Tab 聚焦的 Switch 滚入可见区域，Esc 返回入口。无结果保留清除搜索；Enable all / Disable all 始终操作完整供应商目录，搜索不改变其范围。禁用命中角色默认对时 Host 同批清除该键对并如实提示；目录为空显示诚实空态，不渲染全开假按钮。当 Global `proxy_url` 已配置时，行右侧追加供应商级代理 Switch（OPT-3c 起为 Switch 控件，On/Off 状态词，tooltip 说明走代理/直连）；click / Enter / Space / AX Press 同一 handler，Host `set_provider_use_proxy` 回执即写后状态，不乐观更新。未配置全局代理时不渲染该开关。
- **Network**：Global `proxy_url`；可在 GUI 填写，也可手动写入标准用户配置目录中的 `config.toml`。该文件位于 workspace 外，不会进入仓库；workspace `.pawork/config.toml` 中的代理值会被忽略。未设置显示 `Not set (uses system environment variables)`；新 OAuth / 验证 / 目录同会话生效，当前供应商模型流量于切换或重启后生效。代理是全局开关，供应商级绕过经 Models & providers 页的代理开关表达（`use_proxy = false` 时该 provider 出站直连）。
- **权限与审批**：五档审批模式使用整行 radio，两行说明的行高随字号为 56/70/84px，row click、Enter、Space 与 AX Press 同一 handler；项目信任开关与 Global 默认只读行并列。审批默认与当前 canonical 根路径的信任选择保存到 Global 配置；进行中 Run 不受影响，后续 Run 按实际目标项目取信任。
- **Tools & MCP**：复用 Host `mcp_list`，提供 Test / Remove。
- **Terminal 错误（UX-04）**：终端面板就地解释只读限制、断线、启动中和操作失败，技术详情在输出区展开并自然换行。有效只读权限或当前连接收到的明确只读拒绝禁用创建；权限未知时保留 Host 裁决，stale 权限不作预检依据。恢复入口按实际可用性进入审批设置或连接诊断，不自动降低审批。运行中 I/O 失败仍保留终端可操作状态；错误不写入 Composer。新增按钮支持鼠标、键盘与实际可见框同源的 AX。
- **Terminal**：Global `[terminal]`（shell / columns / rows）；只影响之后创建的终端。
- **Appearance**：Desktop 本地；三档 100%/125%/150% 与 `Cmd+=` / `Cmd+-` / `Cmd+0` 共用 `TextScale`，并显示随选择即时变化的正文 / control 字阶样例；立即生效，保存到用户目录 `desktop.json`，重启恢复选择。主题只读深色，不画 light/system 控件。同页提供语言切换（English / 中文）：两个同源按钮，切换后整界面即时重渲染，保存到同一 `desktop.json`，重启恢复选择。
- **Advanced**：本地连接诊断；断线仍可达。只读 runtime ID、协商 API、capabilities、endpoint、resume/ack，以 definition list 呈现，长路径换行且页尾说明可滚动到达。不展示 GUI token，不从 endpoint 反推 data directory。
- **About**：只在当前认证握手提供非空 `host_data_dir` 时显示，并以 definition list 呈现；缺字段、空白或断线时隐藏并退回 Advanced。路径原样展示，不用于文件操作。

断线保留最后只读结果并统一标 stale；写操作三路径（可见 / 键盘 / AX）同 gate。行为细节以 [Settings Spec](spec/settings.md) 为准。

---

## 4. 协议与分层

GUI 走冻结契约形状（[architecture.md](architecture.md) §3.2）：帧、Command / Query / Event / Snapshot 用完整字段，客户端只消费工作台所需要子集。`--json` 不是 GUI 长期协议。

```text
GPUI view  →  projection（纯 Rust，可从 snapshot+events 重建）
           →  controller（只调 pawork-client）
           →  local transport  →  pawork gui serve  →  app
```

`apps/desktop` 的直接业务依赖只允许 `pawork-client`。`projection` 不导入 GPUI 或 OS API；`platform` 只允许窗口、剪贴板、选工作区目录与拉起固定 `pawork` 二进制。GPUI 锁定精确 revision `=0.2.2`。

#### UI-6a 供应商更新（2026-09-08）

沿用 UI-5 全宽内容与共享控件。供应商卡按内容自然增高，基础内边距 16px，动作高 36px；名称 / 认证方式和连接 / 目录状态分组，UX-06 展开后优先显示账号及额度、账号操作与添加入口，再显示代理、模型管理及不可用 Usage。四默认角色以左侧名称和可换行说明、右侧选择器排列；凭证输入和操作按钮分行，适配窄窗与大字号。

Manage models 弹层宽 320px、最高 400px，显示真实启用数、目录与账号权限的边界说明、批量操作和可滚动模型行。重绘保留滚动位置；默认角色菜单打开时滚入当前项，上下键移动时自动跟随高亮，分组头不计入选择索引。未知 context window 显示不可用，不把远端 0 哨兵显示成 0 容量。供应商页、角色菜单与模型弹层的 AX 读取 GPUI 实测框并裁剪视口外控件，鼠标、键盘与 AX 沿用同一写入 gate。额度无权威数据时仅显示不可用文案与空轨道，不画余额或百分比；多账户属于 UI-6b。

目录与凭证语义见 [ADR-058](spec/settings.md#adr-058ui-6a-目录权威与凭证验证2026-09-08)，验证状态见 [路线图 UI-6a](review/roadmap-ui-2026-09-09.md#ui-6a-本批证据2026-09-08)。

### 4.1 Timeline 恢复

Snapshot 只有会话树、活动 Run、待审批与 Provider 等状态，**没有历史 Timeline 内容**。

1. `SessionGet` 追加可选 `timeline_after_sequence` / `timeline_limit`。
2. `AppResponse::Data` 追加可选 `timeline_page`（`items` / `next_sequence` / `head_sequence` / `complete`）。`items` 由共享 reducer `pawork-protocol::projection` 从已持久化 Agent 事件投影，不暴露 SQLite、Secret 或 Protected Blob 明文。
3. 首连：`snapshot(N) → subscribe(after=N) → 分页取 active session Timeline`。历史页加载期间暂存 live events，到达 `head_sequence` 后按 event id / sequence 去重。
4. 重连得到连续 Replay 时直接续接；得到 `SnapshotRequired` 时丢弃 stale，以新 Snapshot 替换基线并重新分页。Desktop 重启从持久化 Timeline 重建，不能依赖 GUI 本地业务缓存。

旧 minor 不支持 Timeline 页时明确报版本不兼容，不静默显示不完整历史。

---

## 5. 视觉与交互原则

- 原生桌面密度：侧栏窄、主栏宽、Composer 固定在底。不要仪表盘卡片墙。
- 左栏必须提供 Timeline / Projects 两种组织方式；不得恢复占满整行的切换或新建按钮。
- Composer 保持紧凑；ContextMeter、RunStatusBar 与账号页分别表达当前上下文、当前 / 最近 Run 累计 usage、订阅 quota；不把 Run 用量冒充 Session 总量或上下文占用。缺值诚实显示 unavailable。
- 工具调用是 Timeline 里的折叠块，不是单独 IDE 面板。
- 流式输出按 token/事件追加；取消只取消当轮，历史保留。
- 审批 fail-closed：无用户动作不得当默认允许。
- 主题当前仅 dark 基线，不读取系统显示偏好，不构成第二套主题。
- `Enter` 仅在 IME 未组合时发送，`Shift+Enter` 换行；多行粘贴保持原文。
- Timeline 只在用户位于底部时追随流式输出；用户向上阅读后不得抢滚动位置。向上脱钩的滚动区提供回底控件。
- 连接、Run、tool 与审批状态必须有文本/图标语义，不能只靠颜色；主路径可全键盘操作。
- Accessibility 以 Desktop 显式语义树为唯一来源：稳定 identifier 与可本地化 label 分离；macOS 由 AppKit bridge 导出。应用内字号 `Cmd+=` / `Cmd++`、`Cmd+-`、`Cmd+0` 在 100%/125%/150% 间切换。新增可见交互必须同批补语义。当前 UI 无动画，Reduce Motion 无渲染分支。
- 可交互控件必须有 hover 与按下态；hover / active 只改背景，不引起布局移动。 鼠标点击不显示焦点边框；Tab / 方向键导航才显示 2px 中性焦点描边（`text.secondary`），切回鼠标按下立即清除。按钮、列表行、Switch、Settings 导航与 Inspector / Changes 页签共用此规则；业务选中态由背景、下划线或开关位置表达，失焦不改变。
- scope、model、entry、Activity 菜单为 `anchored()/deferred()` 浮层，同一时刻单开互斥；选择 / 再点触发器 / `Escape` / 点击浮层外关闭；打开时滚轮不穿透。Timeline / Projects 直接切换按钮不属于菜单。
- 用户发起的 task 切换、审批决策后焦点回到 Composer；session reset 先关闭旧菜单。
- Timeline 条目经变高虚拟化；侧栏长标题单行省略号截断。
- Resources 只读呈现 MCP 状态，字段缺失显示 unknown；无 Host 出口的分区不画入口。
- `@file` 由 Host 在 run_start 展开为独立 Text part，客户端不本地拼文件内容。

响应式：`1080 × 720` 为功能门禁。OPT-4b 起 Inspector 全部宽度档默认折叠（F6），显式动作才展开；100% 字号时 rail 收敛 240px、中央对话区 ≥560px，150% 时 rail 320px；空间不足（窄窗 / 大字号不足 1320）Inspector 保持折叠，偏好不被 resize 改写。

---

## 6. 插件预留（只留口，不实现）

- Domain 已有 `PluginId`、`ToolCapability::ExternalPlugin`；时间线按普通 tool 事件渲染，不识别插件品牌。
- Snapshot / capability 集合预留扩展位；未知 capability 隐藏，不报错、不画灰掉的市场入口。
- 不激活 `plugin` feature，不建 wasm-host / marketplace 页面。

---

## 7. UI 模块

`apps/desktop/src/ui/`：`mod.rs` 只留 AppView 装配、路由与状态；渲染细节进模块。

| 模块 | 内容 |
| --- | --- |
| `theme.rs` | 色 token + 字阶 + metrics |
| `timeline.rs` | `list()` 变高虚拟化、钉底、跟随/回底 |
| `timeline_entry.rs` | 五类条目 + fork 菜单 |
| `approval_card.rs` | 内嵌审批卡 |
| `input_area.rs` | Composer + model 菜单 + workspace 确认 |
| `inspector.rs` | Changes / Terminal / Resources |
| `recovery.rs` | 连接与终端局部错误、恢复动作、技术详情与 AX |
| `changes.rs` | Files / Summary + DiffView + ActivityPopover |
| `resources.rs` | MCP 只读列表 |
| `task_rail.rs` | 侧栏 + grouping 直接切换 + scope 菜单 |
| `text_input.rs` | Composer 自然换行 TextElement（共享显示行、IME、UTF-16 映射） |
| `settings/` | Settings Rail 与各页 |
| `i18n.rs` | 界面文案目录（English / 中文）与全局语言切换 |
| `accessibility.rs` + `accessibility/` | AX 语义模型与 macOS bridge（ADR-042；语义树来自 AppView 状态） |
| `barriers.rs` | UI fixture barrier 发射器（仅 `PAWORK_UI_BARRIER_DIR` 设置时启用） |
| `shell_layout.rs` | 窗口壳布局合同：三栏宽度、响应式收敛与文本缩放规则的唯一计算入口 |
| `u1_probe.rs` | GPUI TestAppContext 进程内驱动探针（不挂 AppView / socket） |

`ui/components/`：`Button`、`Dropdown`/`MenuPanel`/`MenuRow`、`FollowScroll`/`BackToBottom`、`Label`/`Badge`、`ListRow`、`Panel`、`StatusBar`。scope / model / entry / Activity 菜单全部浮层化，`Option<MenuKind>` 单开互斥；grouping 使用普通 Button 直接切换。


## 8. OPT-D 统一候选稿（已签字）

2026-09-05 按 [OPT-D 归档](review/roadmap-opt-2026-09-05.md#3-opt-d--统一-ui-design闸门) 新增六张 1440×1024 设计稿，覆盖工作台收起/打开、Composer 启用模型菜单、全宽 Settings、供应商展开凭证/代理 Switch/四默认角色，以及模型启用四状态。资产、图标命中区、宽度、状态规则与样例边界统一见 [design/README.md §0](../design/README.md#0-opt-d-统一设计交付2026-09-05已签字)。

用户视觉签字已于 2026-09-05 确认（见 [OPT 归档 §10](review/roadmap-opt-2026-09-05.md#10-状态)）。签字后 OPT-2/3/4 已将新设计替换对应生产合同：OPT-2 会话生命周期、OPT-3 模型启用与默认角色、OPT-4（2026-09-06）落地 Inspector 默认折叠与重开入口、六处主操作图标 36×36/20–22px、Settings 全宽与导航零位移选中态；真窗口对照新图验收另行记录。旧 P0–P2 图继续保留。

OPT-1 行为已同步到 §3.5：审批与项目信任由 Host 保存 Global 配置，语言/字号由 Desktop 保存 `desktop.json`，见 [ADR-053](spec/settings.md#adr-053opt-1-设置持久化2026-09-05)。这种持久化改动不代表 OPT 视觉已落地或已签字。模块重设计 UI-1～UI-6 已实现，全文见 [历史记录](review/roadmap-ui-2026-09-09.md)；OPT-D 签字稿保留为历史资产，不再否决新视觉。当前活动线见 [ROADMAP.md](ROADMAP.md)。

## UI-6b 账号交互更新（2026-09-08）

GUI 1.15 的 Credentials 区沿用 UI-6a 卡片，账号按名称与状态分行：名称/后续请求标记在首行，认证方式/掩码/过期状态在详情行，Use 与 Remove 位于账号内部。名称输入与 Add API key / Add OAuth 沿用 inline 认证流程；空名称禁提交，验证或授权失败保留旧账号与选择。Remove 先进入该行确认，Keep 退出；选中账号仍有其他记录时禁删并解释先选其他账号。列表和动作的实际布局框按 credential ID 绑定，重排不改变删除对象。

“用于后续请求”不表示当前 Run 已切换。env 不列为可删除账号，Usage 保持 unavailable。低于 1.15 的 Host 继续呈现原有操作，不发送新命令。契约见 [ADR-059](spec/settings.md#adr-059ui-6b-命名账号与持久选择2026-09-08)，实现、自动检查与人工验收状态分别见 [路线图](review/roadmap-ui-2026-09-09.md#8-ui-6--providers供应商目录多账号)。


## UI-6b G2 额度交互更新（2026-09-09）

Go 命名账号行下显示 5 小时/每周/每月的已用百分比和距离重置的时间，各账号有刷新按钮；同一订阅的多个 key 不合并、不相加。供应商级“额度耗尽时切换”默认关闭，只有 Host 1.16 且有显式选中 Go API key 才能开启；手动选择恢复关闭。开关在途禁再次发送，回执成功后读取 Host 状态。

刷新时保留旧读数并显示加载，失败立即标过期；读数超过 30 秒、重置时间已到或来源时间异常均标过期。删除、重新登录、离页、断线使旧请求失效。无来源保持 unavailable。刷新与开关复用现有 Button/Switch，AX 按实际按钮框和视口裁剪，支持键盘、三档字号和窄窗。决议见 [ADR-060](spec/settings.md#adr-060ui-6b-g2-逐账号额度与耗尽切换2026-09-09)，验证状态见 [ROADMAP](review/roadmap-ui-2026-09-09.md)。

## UX-06 供应商与账号信息层级（2026-09-09）

已连接供应商优先，同组保持目录顺序；四默认角色不占据首屏。识图 / 搜索在用途说明和当前选择入口都标明「尚未生效」，保存偏好仍使用原命令。账号区按名称与当前选择、认证摘要、三窗额度、刷新 / 使用 / 移除组织；添加账号有独立标题，账号之间以间距和分隔线区分。当前选择用于该供应商后续请求，正在运行的请求保持原账号。

三窗分别展示已有的已用 / 剩余字段，缺失保持未知；倒计时从分钟分解为天 / 小时 / 分钟，标明向上取整到分钟、预计重置或已到重置待确认。来源与获取距今时间保留在对应窗口；超过 30 秒、缓存过期和异常未来时间继续标过期。订阅窗口、多个 Key 的共享额度不可相加；底栏「任务额度 —」表示任务维度尚无读数，不同步账号订阅数值。定向检查已通过，代理真窗口部分通过；真实数值与用户验收边界见 [路线图 UX-06](ROADMAP.md#ux-06-供应商与账号信息层级)。
