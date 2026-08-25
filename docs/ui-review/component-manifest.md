# Pawork Desktop UI 组件 manifest（R1 Wave A）

> 状态：🔒 Wave A 已冻结（2026-08-26 复审修订）· 依据三张 v3 定稿图归一 reference.png（1440×1024）<br>
> 上游任务：[R1 视觉合同](../../plan/R1-ui-visual-contract.md) §2 · [UI 复审](../UI_Review.md) §3/§4/§5<br>
> 本文件回答「每个状态可见什么组件、如何分层与锚定、有哪些状态、每个组件对应什么真实能力」；几何实测值由同波 measurements.md 承载，本文只引用 design/README.md 已声明的定稿值。

## 1. 口径与事实源

- 组件枚举只来自三张 reference.png 的可见内容：`state-a/reference.png`（Timeline + Inspector 展开）、`state-b/reference.png`（折叠 + ActivityPopover）、`state-c/reference.png`（Projects）。
- 能力映射的首选检索入口是 [docs/spec/crates/desktop.md](../spec/crates/desktop.md)（§3 用户可见界面、§4.7 wire 方法表、§5 契约）与 [docs/spec/desktop.md](../spec/desktop.md)（产品边界与只读红线），结论以当前 Desktop/controller/client 源码、冻结 wire 形状与定向测试为事实源；冲突时按源码回写 Spec。design/gui-design 的目标描述不构成 capability 证据。
- 定稿图中的演示数据**不是生产合同**：quota 百分比、token 数、tok/s、Agent 名单（Main/GLM/Grok）、reasoning 档位（GLM-5.3 · High）、附件钮、账户区（Jane Doe）、Add tool「+」、open-in-new、Review changes / Open in editor 等一律按真实 capability 判定；无来源即 hidden / unavailable，禁止写死补图。
- 判定词：**real**（给出 host method / projection 字段）、**partial**（数据源 real 但结构或部分字段缺失，挂 F-xx）、**honest-hidden**（无 capability，不画入口）、**unavailable**（组件在但无权威数据，显 `—` / unavailable 文案）、**结构缺口**（目标组件现实现整体缺失）。
- 状态表四列沿用 UI_Review §5.2 / §8 口径：selected(active) / hover / disabled / hidden。hover 只改背景色（design §8.1 token 表），不引起布局位移。
- 声明几何值引用 design/README.md §2：TaskRail 288px、Inspector ~440px、Composer 常态总高 88–94px、控件行 28–30px、Send 32px、RunStatusBar 24px、ActivityPopover ~320px、8px 基线；实测对照见 measurements.md，冲突记录在该文件冲突表。

## 2. 全局 z-order 与浮层锚点

应用内自上而下的遮挡关系（UI_Review §0.1 交互层硬要求：浮层不遮挡主操作、开合不跳动）：

| 层 | 内容 | 遮挡/开合行为 |
| --- | --- | --- |
| L4 | 浮层族（单开互斥）：GroupingMenu · ScopeMenu · ModelMenu · EntryMenu(fork) · WorkspaceConfirm · ActivityPopover | gpui `deferred(anchored())` + `occlude()`；开新即关旧；选择/再点触发器/Escape/外点关闭；滚轮与点击不穿透；超高在 240px 内自滚 |
| L3 | 区域内浮动件：BackToBottom 回底控件（Timeline / 终端输出区右下） | 绝对定位浮于所属滚动区；贴底跟随时隐藏，脱钩浮出；不跨区域遮挡 |
| L2 | 面板滚动内容：Timeline 虚拟化列表、Files 清单、DiffView、Terminal 输出 | 审批卡是 timeline 列表末项（in-flow），不是浮层 |
| L1 | 三栏骨架：TaskRail 288px ｜ Workspace 弹性 ｜ Inspector ~440px ＋ 底部 RunStatusBar 24px | 分隔线贯通、无双线；Composer 恒在 StatusBar 上方 |
| L0 | macOS 窗口 chrome（traffic lights） | 定稿目标：沉浸深色、与壳一体；现实现为原生白 titlebar（F-01 P0） |

浮层锚点表（L4 逐项）：

| 浮层 | 锚点 | 位置语义 | 现实现差异 |
| --- | --- | --- | --- |
| TR-02 GroupingMenu | TaskRail 标题行右侧 GroupingMenuButton | 向下展开，当前项 checkmark | 一致（浮层形态） |
| TR-04 ScopeMenu | Scope 触发器「All projects ▾」 | 向下展开 | 一致 |
| CP-04 ModelMenu | Composer model 触发器 | 向下，近下缘按 anchored 规则翻转 | 一致；`can_switch_model` 翻假自动关闭 |
| TL-07 EntryMenu | 条目行内「···」钮 | 条目右侧 | timeline reset 前强制关（锚点条目可能被虚拟化卸载，D-04 拍板） |
| CP-11 WorkspaceConfirm | Composer workspace 标签行下方 | 无独立触发器；All projects 范围新建时条件打开 | 一致 |
| PO-01 ActivityPopover | **定稿**：Workspace header 右上 Activity 触发器，向下展开 ~320px，零遮挡 Composer | 折叠态专用 | **D-01 冲突**：现实现锚在底部 StatusBar 右侧、向上展开并覆盖 Composer 右侧 |

## 3. State A — Timeline + Inspector 展开

```text
Window 1440×1024 · L0 traffic lights（目标沉浸深色；现实现白 titlebar，F-01）
├─ TaskRail 288px · L1
│  ├─ TR-01 标题行：Pawork ＋ GroupingMenuButton ◷（Timeline 当前）
│  ├─ TR-03 Scope 筛选：All projects ▾
│  ├─ TR-05 连接行：● Local · Connected ＋ TR-07 全局 AddTaskButton（＋角标）
│  ├─ 滚动列表 · L2
│  │  ├─ TR-08 日期桶 Today
│  │  │  ├─ TR-09 项目头 Pawork_v2 ▾ ［＋］
│  │  │  │  └─ TR-10 Task ● Review GUI architecture（选中行）10:42 AM
│  │  │  ├─ TR-09 项目头 AsterRoute ▾ ［＋］ → Fix replay gap / S7 desktop smoke
│  │  │  └─ TR-09 项目头 Desklet ▾ ［＋］ → Provider cache policy
│  │  └─ TR-08 日期桶 Yesterday → Pawork_v2 / Update docs README …
│  └─ TR-12 账户区：JD · Jane Doe ▾ · ⚙（演示件，无 capability）
├─ Workspace 弹性
│  ├─ WS-01 Header：Review GUI architecture · ⎇ main · ● Completed · 右上动作钮
│  ├─ Timeline 滚动列表 · L2（虚拟化、变高行）
│  │  ├─ TL-01 You · 10:38 AM · 正文
│  │  ├─ TL-02 Pawork · 10:40 AM · 正文段落 ＋ bullet 列表
│  │  ├─ TL-03 工具活动组：3 × (icon · 名称 · ✓ Completed · 48s/61s/35s)
│  │  ├─ TL-04 完成摘要卡：Ready for review ＋ Review changes / Open in editor
│  │  ├─ TL-05 Run completed · 2m 14s · 10:40 AM
│  │  ├─ TL-06 审批卡（pending 才出现；本图无）
│  │  └─ TL-08 回底控件 · L3（脱钩才浮出）
│  └─ CP-01 Composer（常态总高 88–94px）
│     ├─ CP-02 输入框：Ask a follow-up…
│     └─ 控件行：CP-03 GLM-5.3 · High ▾ ｜ CP-05 📎 ｜ CP-06 📁 Pawork_v2 ▾
│                 ｜ CP-07 Context 78K / 128K ＋进度条 ｜ CP-08 Send ⬆
├─ Inspector ~440px · L1（cmd-i 开合）
│  ├─ IN-01 顶层 tabs：Changes(active) · Terminal · Add tool(＋) · ⌒ · ✕
│  └─ IN-02 Changes：Files(4·active) · Summary · ↻ · 汇总行 4 files · +186 −24
│     ├─ IN-04 文件行 ×4：icon · path · M 徽标（docs/gui-design.md 选中高亮）
│     └─ IN-05 DiffView 卡：文件头(path·M·copy····) · 行号槽 · @@ hunk 头
│                       · 增/删行语义色 · 长行横滚
└─ SB-01 RunStatusBar 24px（跨 Workspace＋Inspector 底部，不覆盖左栏账户区）
   Task 92.4K tokens ｜ Z.AI quota 72% left ｜ 38.6 tok/s avg ｜ Run 2m 14s

浮层（L4，本图静态均未打开）：TR-02 / TR-04 / CP-04 / TL-07 / CP-11 / PO-01(仅折叠态)
```

## 4. State B — Timeline + Inspector 折叠 ＋ ActivityPopover

```text
Window 1440×1024 · L0 同 State A
├─ TaskRail 288px · L1 —— 与 State A 完全同构（Timeline 分组：日期→项目→Task）
├─ Workspace 扩展（Inspector 整列隐藏，宽度归零，无分隔线残留）
│  ├─ WS-01 Header：同 A；右上为两个角标钮 —— ＋(Add tool，演示件) 与 ☰(Activity 触发器)
│  ├─ PO-01 ActivityPopover · L4【锚定：右上触发器向下，~320px，不覆盖 Composer】
│  │  ├─ 标题行：Activity ＋ open-in-new 图标
│  │  ├─ Changes 分区：4 files · +186 −24（点击 → 展开 Inspector 定位 Changes）
│  │  └─ Agents 分区：3 行 —— Main agent ✓ Ready for review ·
│  │                              GLM ⟳ Updating layout ·
│  │                              Grok ✓ Review complete（全部为演示数据）
│  ├─ Timeline · L2：同 A（TL-01/02/03/04/05/06/08）
│  └─ CP-01 Composer：同 A
└─ SB-01 RunStatusBar 24px：同 A 指标行

浮层：PO-01 打开（本状态主角）；其余浮层关闭。现实现：PO-01 由底部
SB-02 触发、向上展开并覆盖 Composer 右侧 —— D-01 冲突（F-12 P0）。
```

## 5. State C — Projects

```text
Window 1440×1024 · L0 同 State A
├─ TaskRail 288px · L1 —— GroupingMenu 切到 Projects（folder glyph）
│  ├─ TR-01 / TR-03 / TR-05+TR-07：同 A（顶部结构不变）
│  ├─ 滚动列表 · L2（无日期桶，按 canonical workspace 分组）
│  │  ├─ TR-09 项目头 Pawork_v2 ▾ ［count 4］［＋］
│  │  │  ├─ TR-10 ● Review GUI architecture（选中，蓝点）10:42 AM
│  │  │  ├─ TR-10 ● Fix replay gap 9:18 AM
│  │  │  ├─ TR-10 ● Provider cache policy 8:37 AM
│  │  │  └─ TR-10 ○ S7 desktop smoke 7:55 AM
│  │  ├─ TR-09 项目头 AsterRoute ▾ ［count 2］［＋］（DNS listener hardening / SOCKS5 UDP regression）
│  │  ├─ TR-09 项目头 Desklet ▸ ［count 3］（折叠态：任务隐藏）
│  │  └─ TR-11 Unassigned 桶（缺 workspace_id 的历史 Session；无定向＋）
│  └─ TR-12 账户区：同 A（演示件）
├─ Workspace ＋ Inspector ＋ SB-01：与 State A 完全同构（同一 active session）
```

## 6. 状态表（selected / hover / disabled / hidden）

「—」= 该状态不适用。hover 一律只改背景（token 见 design §8.1）：ghost/角标 → `surface.raised`；raised → `surface.hover`；primary → `accent.hover`；success/danger → 对应 hover token；菜单选中行保持 `accent.primary` 不叠加。

| ID | 出现 | selected / active | hover | disabled | hidden（条件显隐） |
| --- | --- | --- | --- | --- | --- |
| WIN-01 | ABC | — | — | — | 常驻；现实现白 titlebar 与定稿冲突（F-01） |
| TR-01 | ABC | GroupingMenuButton 显示当前模式 glyph | 角标钮 hover | — | 常驻 |
| TR-02 | ABC(关) | 当前项 checkmark（Timeline/Projects） | 菜单行 hover | — | 默认关闭；单开互斥 |
| TR-03 | ABC | 当前 scope（All projects / 某 workspace） | 触发器 hover | — | 常驻 |
| TR-04 | ABC(关) | 当前 scope 项 | 行 hover | — | 默认关闭 |
| TR-05 | ABC | 四态文字：Connected(+resume 文案)/Connecting…/Disconnected·reason/Connect failed·reason | — | — | 常驻；不只靠颜色 |
| TR-06 | 断线 | Primary 按钮 | ✓ | — | 仅断线/失败态显示 |
| TR-07 | ABC | — | 角标 hover | 断线或 stale projection 禁用并给原因 | 否 |
| TR-08 | AB | — | — | — | 空桶不渲染 |
| TR-09 | ABC | — | 头行 hover | 定向＋断线禁用 | 头常驻；折叠时任务隐藏（▾/▸） |
| TR-10 | ABC | 背景＋焦点＋可访问名称三重（目标） | 行 hover | — | 被折叠/被 scope 筛掉 |
| TR-11 | C(缺元数据) | — | 行 hover | — | 仅存在缺 workspace_id 的 Session |
| TR-12 | ABC(演示) | — | — | — | honest-hidden：无账户/设置 capability |
| WS-01 | ABC | 当前会话即 active；Completed=run 终态（点+文字） | 右上角标钮 hover | — | 现实现整块缺失（F-05 P0） |
| TL-01 | ABC | — | — | — | 常驻（会话内容） |
| TL-02 | ABC | 流式中增量合并为一条 | — | — | 常驻 |
| TL-03 | ABC | 每行状态 pending/running/succeeded/failed/cancelled（文字+图标） | 组/行 hover（目标可折叠组） | — | 无 tool 事件的 run 不显示组 |
| TL-04 | ABC | — | 卡片按钮 hover | Review changes / Open in editor 无命令 → 不渲染按钮 | 摘要文案行可由 run 终态组装；终态摘要为 F-08 目标 |
| TL-05 | ABC | — | — | — | 无 run 终态不显示；时长缺权威起止显 `—` |
| TL-06 | pending | — | 三钮 hover | 断线时三钮禁用＋tooltip 原因（fail-closed） | 无 pending 不渲染；run 终态/ApprovalResponded 清卡 |
| TL-07 | 条目菜单 | Fork 行（闭合边界） | 行 hover | 非闭合 run 边界灰字禁用行 | 「···」点击才开；reset 前强制关 |
| TL-08 | ABC | — | ✓ | — | 贴底跟随时隐藏；脱钩浮出；回底/滚到底重挂后隐藏 |
| CP-01 | ABC | — | — | — | 常驻（高度语义见 F-09） |
| CP-02 | ABC | focus 聚焦（启动即聚焦） | — | — | 常驻；IME marked_range 下划线 |
| CP-03 | ABC | 当前 effective model（pending>selected） | 触发器 hover | run 进行中/目录未加载/断线禁用＋tooltip | 常驻 |
| CP-04 | 打开态 | 当前模型项 | 行 hover | 同 CP-03（翻假自动关闭） | 默认关闭 |
| CP-05 | AB(演示) | — | — | — | honest-hidden：无附件 capability |
| CP-06 | ABC | 当前 workspace | 标签行 hover（目标选择器） | — | 常驻；现实现为静态标签非下拉 |
| CP-07 | ABC | — | ✓（Send hover） | can_send=false（断线/无会话/run 中/空文本） | 常驻 |
| CP-08 | run 中 | — | ✓ | — | 空闲态隐藏（⌘. 等价） |
| CP-09 | run 中 | — | ✓（Danger hover） | — | 空闲态隐藏；与 CP-08 成对出现 |
| CP-10 | ABC | — | — | — | 常驻；轮换 status_hint 或禁用原因 |
| CP-11 | 条件 | workspace 项 | 行 hover | — | All projects 范围新建任务时条件打开，无独立触发器 |
| IN-01 | AC | 当前页 raised+下划线（默认 Terminal） | 页签 hover | — | Add tool(＋) honest-hidden（D-02）；✕=折叠 Inspector |
| IN-02 | AC | Files/Summary 当前项；Files 带 count 徽标 | 页签/↻ hover | — | 仅 Changes 页内 |
| IN-03 | AC | — | — | — | 常驻（汇总行） |
| IN-04 | AC | 选中文件行高亮 | 行 hover | — | 无会话/无 diff 显空态文案 |
| IN-05 | AC | 选中文件（与 IN-04 联动） | — | — | binary 显「Binary file — not rendered.」 |
| IN-06 | AC(Summary) | — | — | — | 字段缺失显 unknown |
| IN-07 | AC(Terminal) | 默认页 | Start/Size hover | 未连接时创建失败如实提示 | 懒创建；输出区含回底控件（L3） |
| IN-08 | AC(Resources) | 当前页 | 页签/↻ hover | — | 空列表空态；failed 红字；「已加载规则」不画 |
| IN-09 | AC | — | — | — | 仅查看会话 ≠ latest 数据会话时显示 |
| SB-01 | ABC | — | — | — | 常驻；缺权威值显 `—` 不伪造 |
| SB-02 | ABC | 展开态「Hide inspector」/折叠态触发 PO-01 | ✓ | — | 常驻（现实现位置与定稿冲突 D-01） |
| PO-01 | B | Changes 摘要可点击（展开定位） | 摘要行 hover | — | 仅折叠态可开；Agents 分区 honest-hidden（S11 前） |

## 7. 能力映射表（real / partial / honest-hidden / unavailable / 结构缺口）

| ID | 判定 | 依据（host method / projection 字段 / 缺口） |
| --- | --- | --- |
| WIN-01 | 结构缺口 F-01 | 目标沉浸深色 titlebar＋traffic lights 安全区；现实现 macOS 原生白 titlebar（`run_app` 窗口配置）。无 wire 依赖 |
| TR-01 | real | GroupingMenuButton 本地 presentation preference；分组由 `DesktopProjection`（session_tree→日期桶/项目分组）本地计算，无 wire 调用 |
| TR-02 | real | `MenuKind::Grouping` 浮层；切换不改 active session / 草稿 / run（crates spec §3.2/§4.4） |
| TR-03 | real | scope 选项来自 snapshot workspaces（`WorkspaceSummary` 投影）；本地筛选 |
| TR-04 | real | `MenuKind::Scope` 浮层 |
| TR-05 | real | `ConnectionState`/`ResumeState`；握手＋resume＋`subscribe_all`；Replay/SnapshotRequired/UpToDate 三态文字落侧栏 |
| TR-06 | real | Reconnect 重走 `start_connect`（带 last_acked resume）；ADR-026 断线不取消 Run |
| TR-07 | real | `session_create` → `Snapshot`+`SessionCreated`；All projects 范围先经 CP-11 选定 workspace |
| TR-08 | real | 日期桶 Today/Yesterday/Previous 7 days/Earlier 由 `SessionSummary` 时间投影 |
| TR-09 | real | canonical `workspace_id` 分组＋`WorkspaceSummary` 元数据；Projects 模式 count 徽标；定向＋绑定该 workspace；断线禁用并解释 |
| TR-10 | real | `select_session` → `session_get`（`timeline_after_sequence`/`timeline_limit` 分页)；运行点 ● = snapshot `active_runs`；相对时间 now/Nm/Nh/Nd |
| TR-11 | real | 缺 `workspace_id` 的 Session 进 Unassigned，无定向＋，不静默丢失 |
| TR-12 | honest-hidden | 无账户/设置 wire 或 projection 字段；现实现仅「Local」页脚；JD/Jane Doe/⚙ 均为图上演示数据 |
| WS-01 | 结构缺口 F-05 | Header 整块缺失。字段来源：标题←`SessionSummary`；branch←diff 响应 git 信息（现仅 Changes·Summary 页显示）；Completed←`RunChanged` 终态；右上触发＝`ToggleInspector`（现居 SB-02） |
| TL-01 | real（结构缺口 F-07） | `session_get` → `timeline_page.items`（`TimelineEntryKind` user）；`@`token 由 host 在 `run_start` 展开为独立 Text part |
| TL-02 | real（结构缺口 F-07） | `AssistantDelta` 按 message_id 增量合并；发言人/时间戳/段落列表的视觉层级为重构目标 |
| TL-03 | real（结构缺口 F-08） | `ToolStarted`/`ToolOutput`/`ToolCompleted` 事件（name·status+输出摘要）；活动组卡与逐 tool 时长为视觉目标，时长需权威起止 |
| TL-04 | partial | 文案行可由 run 终态＋`ActiveRun` 组装；**Review changes / Open in editor 无任何 wire 命令** → 不画假按钮（S11 前不可用） |
| TL-05 | real | `RunChanged` 终态＋`ActiveRun` 起止＋UI 注入 `now_ms`；运行中 mm:ss、空闲 idle、缺时间戳 `—` |
| TL-06 | real | snapshot `pending_tool_approvals`＋`ToolApprovalRequired` → `tool_approve`（approve_once/approve_for_run/deny）；fail-closed，断线禁用 |
| TL-07 | real | `session_fork`；渲染层＋入口双层 gate（`is_fork_boundary`）；不可用灰字禁用行 |
| TL-08 | real | `FollowScroll`/`BackToBottom` 组件（贴底判定/脱钩/重挂） |
| CP-01 | real（几何缺口 F-09） | Composer 结构在；88px 被现实现用作输入框最小高而非常态总高 |
| CP-02 | real | `TextInput`（IME `marked_range`/UTF-16 映射/多行钳制）；Enter→`run_start`（is_composing 裁决） |
| CP-03 | real | `model_list` → `ModelsLoaded`；`effective_model`=pending>selected，只影响下一轮；显示 provider/display_name。图上「· High」reasoning 档位无 wire 字段 → 演示值 |
| CP-04 | real | `MenuKind::Model`；`can_switch_model` 翻假时由 render 关闭 |
| CP-05 | honest-hidden | 无附件 capability（附件属 S9+ Composer 扩展） |
| CP-06 | partial | 现实现为静态 workspace 标签（projection 元数据）；无 workspace picker 命令；图上 chevron 为演示形态 |
| CP-07 | partial | 分母 real：catalog `context_window_tokens`；分子（当前请求权威 token estimate）无 wire 来源 → 现显示 `Context · — / {window}` 或 `Context · unavailable`；进度条 honest-hidden |
| CP-08 | real | `run_start`；`can_send`=Connected＋active session＋无 run＋非空文本 |
| CP-09 | real | `run_cancel`（⌘.）；run 终态收敛 |
| CP-10 | real | `status_hint` / 禁用原因文案（本地投影＋controller 状态；含 resume 三态文案） |
| CP-11 | real | `resolve_new_task_workspace` 判定 → 选定后 `session_create` |
| IN-01 | partial | `InspectorTab` 本地 UI 态（默认 Terminal，cmd-i 开合，各页滚动独立）；**Add tool 无 surface 注册 capability → honest-hidden（D-02）**；✕ 等价折叠 real；⌒ 用途未定义不画 |
| IN-02 | real | Files/Summary 二级 tab＋↻ 手动刷新（`ChangesPanelState` epoch 防过期） |
| IN-03 | real | `diff_list_files` → `DiffFilesLoaded`（带 epoch）；聚合 `N files · +A/−D` |
| IN-04 | real | `DiffFileSummary`（path·status·+A/−D）；点击选中 → `diff_get` |
| IN-05 | real | `DiffFileDetail`/`DiffHunkDetail`/`DiffLineDetail`；hunk 语义着色＋`overflow_x_scroll`＋binary 标注；文件头 copy/··· 无命令 → 不画 |
| IN-06 | real | Summary 七字段（Session/Files/Lines/By status/Branch/Dirty files/Work dir），缺失显 unknown |
| IN-07 | real | `terminal_create`/`terminal_write`/`terminal_resize` → `TerminalCreated`＋流式 `TerminalOutput`；cwd 限 workspace 相对路径；滚动文本非本地 PTY |
| IN-08 | real | `mcp_list` → `McpServersLoaded`；只读（name/transport/state/tools/last_error 诚实显示）；「已加载规则」无 Host 出口不画 |
| IN-09 | real | host `diff_*` 固定解析 latest 会话；`session_mismatch` 时 banner＋popover 提示行如实标注 |
| SB-01 | partial | tokens/quota/tok/s 无权威 wire 来源 → 一律 `—`（图上 92.4K/72%/38.6 均为演示值，非合同）；Run duration real（`ActiveRun` 起止＋`now_ms`；idle/—） |
| SB-02 | real | `ToggleInspector`（本地动作）；位置与定稿右上触发冲突 → D-01 |
| PO-01 | partial | Changes 摘要 real：`diff_list_files` 聚合（`N file(s) · +A/−D`，未拉取/不可用显 unavailable 不显 0）；点击展开 Inspector 定位 Changes；latest 会话提示行 real。**Agents 分区 honest-hidden**（S11 多 Agent projection 不存在）；open-in-new 无对应动作不画；锚定冲突 D-01 |

组件计数：45 条（WIN 1 · TR 12 · WS 1 · TL 8 · CP 11 · IN 9 · SB 2 · PO 1）；其中浮层 6（TR-02/TR-04/CP-04/TL-07/CP-11/PO-01）。

## 8. 附录：设计文档回写记录（D-01/D-02/D-03，已裁定）

> 以下三项已于 2026-08-26 经用户按主代理建议拍板，并同步到 [design/README.md](../../design/README.md)、[docs/gui-design.md](../gui-design.md) 与必要的 Desktop Spec。这里保留裁定内容与回写落点，不再作为待批准提案。

### D-01 ActivityPopover：右上（定稿） vs 底部 StatusBar（现实现）

**已回写 design/README.md §5.1 / §8.5：**

> Activity 触发器固定位于 Workspace header 右上（Inspector 折叠态常驻角标）；ActivityPopover 自触发器向下展开，宽约 320px，不得覆盖 Composer、ContextMeter 或审批主操作。底部 StatusBar 触发器是 apps/desktop 2026-08-24 波 D 的历史实现记录，不作为视觉验收依据；迁移到右上锚定前，UI_Review F-12 保持未通过。

**已回写 docs/gui-design.md §3.3：**

> 折叠态 ActivityPopover 的触发器随 Workspace header 落位右上，不由 StatusBar 承载；StatusBar 只保留状态信息。Popover 内 Agents 分区在 S11 多 Agent projection 接通前隐藏，不画演示名单。

### D-02 Inspector 顶层 tabs：定稿 Add tool vs 现实现 Resources

**已回写 design/README.md §5 / §8.5：**

> Inspector 顶层 tab 的定稿形态是 capability-driven 的 InspectorToolTabs：Changes、Terminal 与已注册 Inspector surface。「Add tool」只管理已注册 surface，仅在 Host 提供注册 capability 后出现；Resources 是随 S9 接入的只读 surface 实例，其存在不等于 Add tool 定稿形态已达成。§8.5 记录的固定三页签（Changes/Terminal/Resources）为过渡期实现记录；surface 注册路径定稿并落地前 F-10 保持未通过，不画假 Add tool，也不把 Resources 视为已对齐。

**已回写 docs/gui-design.md §3.3 / §5（S9 行）：**

> Resources 页签按 S9 MCP 只读接入；它与定稿「Add tool」的关系是「已注册只读 surface 的首个实例」，不得以固定 Resources 页签替代 Add tool 入口参与验收。

### D-03 1080×720 响应式门禁 vs 1440×1024 像素门禁

**已回写 design/README.md §7：**

> 验收分两级：1440×1024 三张定稿状态执行像素级 99% 门禁；1080×720 执行响应式**功能**门禁，不与 1440 截图做像素对比——1080–1279 宽度下 TaskRail 收敛 240px、Inspector 默认折叠为抽屉、中央对话区不小于 560px，Composer、RunStatusBar 与 Inspector 触发器可用，无裁切、遮挡、状态栏溢出或不可达主操作；Connected 态与断线等边界态均须取证（DESK-10）。

**已回写 docs/gui-design.md §6：**

> 1080×720 为响应式功能门禁：验证主操作可达、焦点可见与布局不溢出；不参与 1440×1024 定稿图的像素对照，也不得以固定宽度溢出为由降低可用性。
