# Pawork Desktop GUI 设计

> 本文是 Desktop GUI 的**设计事实源**（S7 波 0 于 2026-08-16 锁定信息架构；历史组件化完成不代表当前视觉已验收）。当前实现按新 R1–R8 主线还原既有壳，不另起一套信息架构。
>
> 视觉实施基准：[../design/README.md](../design/README.md)（定稿图、TaskRail 双分组与响应式约束）
>
> 关联：[spec/desktop.md](spec/desktop.md)（当前 Desktop 产品/验收汇总，非视觉事实源）· [spec/crates/desktop.md](spec/crates/desktop.md)（包级 Spec）· [../ROADMAP.md](../ROADMAP.md) · [UI Review](UI_Review.md) · [R1 收口存档](history.md#r1--视觉合同固定-fixture-与-ui-测试基座2026-08-2527) · [R7–R8 任务书](../plan/R7-R8-ui-quality-gates.md) · [history.md](history.md)（S7/S13 与旧 R8 交付原委）· [references.md](references.md) · 根仓 [Desktop GUI](../../Pawork_v1/docs/features/desktop-gui.md) · [GUI 连接](../../Pawork_v1/docs/features/gui-connection.md) · [ADR-035](../../Pawork_v1/docs/adr/ADR-035-gpui-desktop.md)

---

## 1. 目标与非目标

**目标**：先交付一个能真实驱动 `pawork` 的最小 Agent 窗口——选会话、看时间线、发消息、取消当轮、切换已配置模型。界面从最简模型长出，而不是按 V1 Phase 19 一次性铺满 Settings / Diff / Terminal / Workflow。

**非目标（本设计明确不做）**：

- 不嵌入 Core，不直连 Provider / SQLite / 工具 / Keychain。
- 不做 TUI，不做 WebView / JS 壳。
- 不实现插件市场、Hooks 管理、WASM 安装器（整族未排期，见 [ROADMAP §5 候选池](../ROADMAP.md)）。
- 不在 S7 做完整多窗口远程桌面、签名安装器、主题生态。

架构红线沿用根仓：独立 GPUI 进程，只经 GUI Connection Protocol 连接 CLI；关闭窗口不取消已进入 Core 的 Run。

---

## 2. 参照与取舍

对照现有 Agent GUI，只吸收可验证的「主对话壳」行为，不复制完整 IDE 或竞品视觉；Pawork 对自己的 v3 定稿图仍执行 [99% 一致性合同](UI_Review.md#01-99-一致性的硬定义)。下表按 2026-08-17 的官方公开资料核对：

| 参照 | 吸收 | 不吸收 |
| --- | --- | --- |
| [Codex](https://github.com/openai/codex)（[app](https://openai.com/index/introducing-the-codex-app/)） | 项目内组织 thread、thread 内持续查看 Agent 进度与结果，桌面与 CLI 会话连续 | 多 Agent command center、Worktree 编排、Skills / Automations、Cloud / Remote 全家桶 |
| [OpenCode](https://opencode.ai/)（[models](https://opencode.ai/v2/docs/models) / [tools](https://dev.opencode.ai/docs/tools/)） | 会话继续、当前会话模型切换、工具详情与 permission 状态可见 | TUI 键位、WebView/JS 插件面板、并行多会话工作站 |
| [DeepSeek Harness](https://deepseek.com/harness) Web UI | Trajectory / 仅追加会话回放、工具与审批留在同一对话 | Cordis/JS 插件面板、Web-first 默认壳、Code Mode 编辑器 |
| [Cursor Agent / MCP](https://docs.cursor.com/context/model-context-protocol) | 工具请求、参数与结果在对话内可展开；需要时就地审批 | 编辑器分屏、代码导航、IDE Settings 与 MCP 管理面 |
| [Zed Agent Panel](https://zed.dev/docs/ai/agent-panel) / [Parallel Agents](https://zed.dev/docs/ai/parallel-agents) | Thread 按项目分组、项目头定向新建、Changes 摘要、Agent 状态；模型选择与 token usage 靠近 Composer | 完整编辑器面板布局、worktree 与 Terminal Thread 管理 |
| V1 [desktop-gui.md](../../Pawork_v1/docs/features/desktop-gui.md) | `ui / projection / controller / platform` 四层；Snapshot + `global_sequence` Replay | P19-1～P19-16 一次铺开 11 个 Surface |

S7 的产品形状：**一个本地 Coding Agent 聊天窗**，不是工作站。Git / MCP / 多客户端 / Plan 等能力随后续阶段长到同一壳上。

---

## 3. 最小信息架构（S7 只做这些）

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

| Surface | S7 范围 | 明确延后 |
| --- | --- | --- |
| Connection / Shell | 发现或拉起本机 `pawork gui serve`、连接状态、断线提示 | 多 instance、远程 Host、updater |
| TaskRail / Sessions | 列表 / 新建 / 打开 / resume；同一 Session 集合可按时间或项目组织 | Fork / 分支树 |
| Timeline | user、assistant 流式、tool 调用起止、错误 | citation、Artifact 分页、thinking 精细折叠（有事件就只读展示，不做专门产品页） |
| Composer | 纯文本发送、取消当轮、下拉已配置 model/provider | `@file`、附件、profile（S9 再长） |
| Approval | 时间线内嵌仅本次允许 / 本轮运行允许 / 拒绝（复用 S3 语义） | 完整 Policy 说明页、信任向导 |
| Changes / Terminal / Settings / Resources / Workflow | 占位或隐藏 | 分别随 S8–S11 增量 |

空态：无会话时主区只有一句提示和 Composer。不以假卡片冒充未实现能力。

### 3.1 主路径与状态

S7 的唯一主路径是：启动 Desktop → 连接本机 Host → 恢复 Snapshot 与当前会话历史 → 新建或打开会话 → 发送一轮 → 查看流式文本 / 工具活动 → 必要时审批或取消 → 在终态继续下一轮。一个窗口同一时刻只操作一个 active session；其它会话只在侧栏显示摘要与运行状态。

| 状态 | 必须显示 | 可用动作 |
| --- | --- | --- |
| 首次连接 / 无可用 Host | 正在连接；失败后显示可重试的原因 | 重试或退出；不显示业务假数据 |
| 已连接、当前会话空闲 | 权威 Timeline、当前 model/provider | 发送、新建/打开会话、切换模型 |
| 当前 Run 运行中 | assistant 流式片段；tool `pending/running/succeeded/failed/cancelled`；明确的运行中状态 | 取消当轮；发送与模型切换禁用 |
| 等待审批 | 内嵌工具名、目标、风险与短摘要 | `仅本次允许` / `本轮运行允许` / `拒绝`；仍可取消当轮；无默认允许 |
| Run 已完成 / 失败 / 取消 | 终态留在 Timeline，不删除已产生内容 | 继续下一轮；失败信息可复制 |
| 重连中 | 保留内存 projection 但整体标为 stale / 只读 | 禁用发送、模型切换与审批；不得把本地 pending 当权威结果 |
| 协议不兼容 | 明确显示客户端与 Host 版本不兼容 | 只允许退出/重试；不得降级走 `--json` |

模型选择器只列 Host 返回的已配置条目；切换只影响下一轮，并以 Core 的确认事件覆盖本地 pending。审批按钮严格映射 S3 的 `ApproveOnce / ApproveForRun / Deny`；关闭审批卡片不能等价于允许。

### 3.2 TaskRail：按时间 / 按项目

选定视觉基准为 [Timeline](../design/desktop-shell-timeline-v3.png)、[Timeline · Inspector 折叠](../design/desktop-shell-timeline-collapsed-v3.png) 与 [Projects](../design/desktop-shell-projects-v3.png) 三个状态；完整布局、token、响应式与验收规则见 [design/README.md](../design/README.md)。后续 GUI 实现默认参照这组设计。

- 顶部 `All projects / <project>` 是范围筛选；`Timeline / Projects` 是分组方式。两者正交，不能把已选项目的下拉框同时当作分组开关。
- 分组方式收进 Pawork 标题行右侧的 `GroupingMenuButton`：Timeline 使用 clock/list glyph，Projects 使用 folder/list glyph；点击菜单切换，不保留宽幅 segmented control。
- `Local · Connected` 右侧的全局 `AddTaskButton` 不保留全宽样式；Timeline 与 Projects 的每个项目头另有 `ProjectAddTaskButton`，新 Task 默认绑定该项目的 canonical `workspace_id`。断线与 stale projection 时两类入口均禁用并解释原因。
- Timeline 层级固定为日期 → 项目 → Task：日期按 Today / Yesterday / Previous 7 days / Earlier，日期内项目与项目内 Task 均按最近活动倒序；Task 行不再重复项目名。
- Projects 按 canonical Workspace 项目分组，项目头同时显示 task count 与定向新建角标；缺失项目元数据的 Session 进入 `Unassigned`。
- 切换分组不改变 active session、Composer 草稿、Run 或主 Timeline；当前 Session 在新组织方式下滚动到可见位置。
- 分组方式、范围筛选和项目展开状态是本地 presentation preference，不新增 Agent domain 事件，也不改变 GUI Connection Protocol。

### 3.3 Context、运行信息与 Inspector 工具位

- Composer 常态高 88–94 px，同行控件高 28–30 px；模型 / reasoning 只在模型选择器显示。工作目录与 Send 之间显示 `ContextMeter`：当前请求上下文估算 / model catalog context window。容量未知时显示 unavailable，不用 Session 累计 token 冒充。
- Workspace 与 Inspector 底部共享 24 px `RunStatusBar`，按优先级显示 Task 累计 token、Provider 剩余额度、output tokens/s 与 Run duration；不重复 Composer 的模型 / reasoning。字段没有权威来源时显示 unknown / `—`，不伪造数值。
- Inspector 顶部预留 capability-driven `InspectorToolTabs`；Changes 是 S8 surface，Terminal 是 S10 surface，Files / Summary 仍是 Changes 内部二级 tab。折叠时 Inspector 宽度归零，Workspace 扩展，右上 `ActivityPopover` 摘要显示 Changes 行数与 Main / subagent 状态；点击摘要恢复对应 Inspector surface。折叠态 ActivityPopover 的触发器随 Workspace Header 落位右上，不由 StatusBar 承载；StatusBar 只保留状态信息。
- ActivityPopover 的 Changes 分区随 S8 启用，Agent 状态列表随 S11 启用；不可用阶段隐藏对应分区，不做可点击假入口或截图演示数据。
- 上述展示只消费 projection / Host capability，经 controller → `pawork-client` 获取；GUI 不直连 Provider、quota store、Git、PTY 或数据库。
- R6 Wave B 已用真 Host/Desktop 九场景验证 Changes/Terminal/Resources、折叠恢复、task/latest-session scope 与断线重连；DiffView 横滚由真实 CGEvent 产生可观测 offset。该结构/交互证据不替代 R8 的三图 SSIM 与用户视觉签字。

---

## 4. 协议与分层（S7 最小切片）

GUI 仍走冻结契约形状（[architecture.md](architecture.md) §3.2 GUI 协议）：帧、Command / Query / Event / Snapshot 用 V1 完整字段，S7 **只消费**对话所需子集。

| 切片 | S7 必做 | S10 再补 |
| --- | --- | --- |
| Transport | 本机 Unix socket / Named pipe | remote TLS、memory 测试矩阵 |
| Handshake | 版本协商、单客户端、本机身份 | 多客户端 capability、慢客户端隔离 |
| Query | workspace/model 列表；`SessionGet` 的追加式分页 Timeline；Snapshot 基线提供 SessionTree / ActiveRuns / PendingToolApprovals / ProviderStatus | artifact 分片、usage、workspace index |
| Command | create/resume session、submit turn、cancel、approval | fork、service、PTY resize |
| Event | message/tool/run/approval/error + `global_sequence` | 全量 Hub 订阅面 |
| Replay | 重连后从 last-ack `global_sequence` 补事件；补不齐则重新 Snapshot 并重取当前会话 Timeline | 多客户端一致性、慢客户端隔离与 protocol-probe 全矩阵 |

### 4.1 Timeline 恢复决策

V1 Snapshot 只有会话树、活动 Run、待审批与 Provider 等状态，**没有历史 Timeline 内容**；S7 不得把不存在的 `snapshot(session+timeline)` 当作可迁资产。波 A 按以下方式补齐，且不新增同 major 的枚举变体：

1. `SessionGet` 保留现有变体，在协商后的新 minor 中追加可选请求字段 `timeline_after_sequence` / `timeline_limit`。
2. `AppResponse::Data` 保留现有信封与 Session 数据形状，只追加可选 `timeline_page`：`items`、`next_sequence`、`head_sequence`、`complete`。`items` 是由共享 reducer `pawork-protocol::projection`（R3 波 C 下沉，host 经 `pawork-app::gui_server` 装配提供）从已持久化 Agent 事件投影出的 presentation-safe 条目，不暴露 SQLite、Secret 或 Protected Blob 明文。
3. 首连走 `snapshot(N) → subscribe(after=N) → 分页取 active session Timeline`。历史页加载期间 controller 暂存 live events；到达 `head_sequence` 后按 event id / sequence 去重，再交给 projection。
4. 重连得到连续 Replay 时直接续接；得到 `SnapshotRequired` 时丢弃 stale 权威标记，以新 Snapshot 替换基线并重新分页当前会话。Desktop 重启同样从持久化 Timeline 重建，不能依赖 GUI 本地业务缓存。

上述字段属于 ADR-036 允许的 optional field 演进：波 A 必须 bump minor 并先锁 golden；旧 minor 不支持 Timeline 页时，S7 Desktop 明确报版本不兼容，不静默显示不完整历史。

### 4.2 进程与依赖边界

进程分层不变：

```text
GPUI view  →  projection（纯 Rust，可从 snapshot+events 重建）
           →  controller（只调 pawork-client）
           →  local transport  →  pawork gui serve  →  app-service
```

`apps/desktop` 的直接业务依赖只允许 `pawork-client`；GPUI 与纯 UI 辅助库不算业务依赖。它不得直接依赖 `pawork-app` / `pawork-engine` / `pawork-providers` / `pawork-storage` / `pawork-tools` / `pawork-git` / `pawork-protocol`（R1 后现名，断言见 `apps/desktop/src/platform.rs`）；该 deny list 在波 B/C 用 `cargo metadata` 实测。`projection` 不导入 GPUI 或 OS API，`controller` 只调 client，`platform` 只允许窗口、剪贴板、选工作区目录与拉起固定 `pawork` 二进制。GPUI 依赖在创建 crate 时锁定精确 revision。

S1 起的 `--json` 仍标 **unstable**。S7 的 GUI **不**把 `--json` 当长期协议；最小 gui-protocol 激活后，Desktop 只走正式帧。S10 再把 `--json` 对齐 headless，并补 SDK / ACP / 多客户端。

---

## 5. 随阶段增量（同一窗口，不换壳）

| 阶段 | Core 新能力 | GUI 增量（只加面，不改四层） |
| --- | --- | --- |
| S7 | 最小 `gui serve` + 单客户端协议 | 本设计的 Agent 壳：日期内项目分组 TaskRail / 紧凑 Composer / Context / 取消 / 模型 / 审批按钮；状态栏只显示已有权威字段 |
| S8 | diff / checkpoint / rollback | InspectorToolTabs 激活 Changes；折叠态 ActivityPopover 显示文件数与增删行摘要 |
| S9 | MCP / AGENTS.md / `@file` | Composer `@` 补全；Resources 只读：MCP 列表、已加载规则 |
| S10 | 正式协议 / 多客户端 / Fork / PTY / service | 重连 Replay、Fork、InspectorToolTabs 激活 Terminal；本机多窗口未做（ROADMAP 候选池） |
| S11 | Plan / 后台任务 / usage / 多 Agent | Workflow 与完整用量/quota 状态条；ActivityPopover 激活 Main / subagent 状态列表 |
| S12 | 全项目 Code Review | 只读核对 Desktop 四层边界、状态投影、能力声明、可访问性及 S7–S11 GUI 需求/证据；不改界面、不启动窗口 |
| 待决策 | WASM 插件 / Hooks / LSP / 市场 | 预留 Resources 空位与协议扩展点，**不画假市场页** |

后续阶段任务书必须带一行「GUI 增量」；没有对应投影/命令就不做按钮。

---

## 6. 视觉与交互原则

- 原生桌面密度：侧栏窄、主栏宽、Composer 固定在底。不要仪表盘卡片墙。
- 左栏必须通过标题行角标菜单提供 Timeline / Projects 两种组织方式；Timeline 使用日期 → 项目 → Task，连接行提供全局新建，项目头提供定向新建。实现前对照 [视觉实施基准](../design/README.md)，不得恢复占满整行的切换或新建按钮。
- Composer 保持紧凑；ContextMeter 与 RunStatusBar 必须区分当前上下文、Session 累计 usage、quota、tokens/s 与 Run duration。模型 / reasoning 只在 Composer 选择器出现，缺值诚实显示 unavailable，不能用推断值填满界面。
- Inspector 顶层工具 tab 与 Changes 内 Files / Summary 二级 tab 必须保持层次；折叠态只用 ActivityPopover 呈现可操作摘要，Surface 未接通时不画可点击假入口。固定 Resources 页签是过渡实现记录，是「已注册只读 surface 的首个实例」，不视为定稿 Add tool 入口的达成。
- `1080 × 720` 为响应式功能门禁：验证主操作可达、焦点可见与布局不溢出（rail 收敛 240px、Inspector 默认折叠、中央对话区 ≥560px）；不参与 `1440 × 1024` 定稿图的像素对照，也不得以固定宽度溢出为由降低可用性。
- 工具调用是 Timeline 里的折叠块（名字、状态、短摘要），不是单独 IDE 面板。
- 流式输出按 token/事件追加；取消只取消当轮，历史保留。
- 审批 fail-closed：无用户动作不得当默认允许。
- 主题当前仅 dark 基线一套实现（取值见 [视觉实施基准](../design/README.md) §8，残余补齐见 §8.6），不跟随系统 light/dark，light 支持顺延后续阶段；S7 不做主题市场。
- `Enter` 仅在 IME 未组合时发送，`Shift+Enter` 换行；多行粘贴保持原文。
- Timeline 只在用户位于底部时追随流式输出；用户向上阅读后不得抢滚动位置。
- 连接、Run、tool 与审批状态必须有文本/图标语义，不能只靠颜色；主路径可全键盘操作。
- Accessibility 以 Desktop 显式语义树为唯一来源：稳定 identifier 与可本地化 label 分离，role/value/enabled/focused/selected/bounds/action 随 canonical UI 状态同步；macOS 由 ADR-042 AppKit bridge 导出，AX action 回到既有 AppView handler 与 enable gate。新增可见交互必须同批补语义；Windows/Linux 平台实现仍属后续阶段。R7 Wave A 于 2026-08-30 依用户决定以原生 AX tree/action + 纯键盘 + U2 替代该波 VoiceOver；VoiceOver 未执行且屏幕朗读措辞 / 顺序未验证，R8 系统级验收不由此自动豁免。
- 可交互控件必须有 hover 反馈与按下态，色值经 theme token；hover / active 只改背景，不引起布局移动（旧 V3 R8 波 B 起，取值表见 [视觉实施基准](../design/README.md) §8.1）。
- 菜单为 `anchored()/deferred()` 浮层，不占布局流；同一时刻单开互斥，选择 / 再点触发器 / `Escape` / 点击浮层外关闭，打开时滚轮不穿透到下层滚动容器（形态细则见基准 §8.2）。
- 焦点交接必须可预测：用户发起的 task click / Enter / AX press / cycling / next-needs-attention 切换、审批决策和 Fork 接受后回到 Composer；Review changes 展开 Inspector 后落到当前选中的 Changes 顶层页签；任何 session reset 先关闭旧菜单。AXPress 当前 task 仍须关闭菜单并聚焦 Composer；仅一个可见 task 时 cycling 不重开 session，但仍须交接焦点。R7 Wave B 已以导航 26 相位、审批/状态 14 相位及审查边角 3 相位真窗口 U2 验证这些路径（[证据](ui-review/r7-wave-b/notes.md)）。
- 用户向上滚动脱钩跟随的滚动区（Timeline / 终端）提供回底控件，点击或自行滚到底即重挂跟随（基准 §8.3）。
- Timeline 条目经变高虚拟化渲染，长会话滚动性能不随长度退化；侧栏长标题单行省略号截断（基准 §8.4，旧 V3 R8 波 C 起）。
- Resources 页只读呈现 MCP 状态（name / transport / state / tools / last_error），字段缺失显示 unknown，不伪造；无 Host 出口的分区（如已加载规则）不画入口（基准 §8.5，旧 V3 R8 波 D 起）。
- `@file` 引用由 Host 在 run_start 展开为独立 Text part，客户端不本地拼文件内容；Timeline 用户消息按 parts 顺序拼接渲染，与 CLI 历史语义一致（基准 §8.5，旧 V3 R8 波 D 起）。

---

## 7. 插件预留（只留口，不实现）

GUI 与协议现在就要避开「以后为插件推倒重来」：

- Domain 已有 `PluginId`、`ToolCapability::ExternalPlugin`；时间线按普通 tool 事件渲染即可，不识别插件品牌。
- Snapshot / capability 集合预留扩展位；未知 capability 隐藏，不报错、不画灰掉的市场入口。
- 不激活 `plugin` feature（`pawork-domain` 的空锚 `plugin = []`）、不建 wasm-host / marketplace 页面。
- 决策记录见 [ROADMAP §5 候选池](../ROADMAP.md)。

---

## 8. 验收（设计锁定）

> 2026-08-16 · S7 波 0 已锁定。这里验收的是设计与可检查规则；真实依赖图已在波 B/C 用 `cargo metadata` 实测（desktop 直接业务依赖仅 `pawork-client`）。v3 TaskRail 已于波 D 落地（日期→项目→Task / GroupingMenu / 定向新建）；1440×1024 人工对照定稿图未做。

- [x] 对照 §2 写明吸收/不吸收，并与本节信息架构一致。
- [x] S7 实现范围不超过 §3；后续阶段只按 §5 加面。
- [x] 四层边界与「不链 Core」已形成可执行 deny list；实测留波 B/C。
- [x] 插件/市场/Hooks/LSP 无产品入口，仅有隐藏扩展点。
- [x] 方案 2 v3 已形成日期内项目分组、项目定向新建、紧凑 Composer、去重 RunStatusBar，以及 Inspector 展开 / ActivityPopover 折叠状态的视觉实施基准。

---

## 9. V3 组件清单（旧 V3 R8 收口，2026-08-24）

`apps/desktop/src/ui/` 终态形态（19 文件 / 5144 行）：`mod.rs` 只留 AppView 装配、路由与状态（1031 行；已拍板接受为终态，见附录 A.3 D1），渲染细节全部进模块。

| 模块 | 内容 |
| --- | --- |
| `theme.rs` | 29 色 token（bg/surface/border/text/accent/semantic 六组 + hover 系）+ 字阶（11/12/13 + `font::MONO`=Menlo）+ metrics |
| `timeline.rs` | Timeline 容器：gpui `list()` 变高虚拟化、`ListAlignment::Bottom` 钉底、跟随/回底重映射 |
| `timeline_entry.rs` | 五类条目渲染 + 「···」fork 菜单 |
| `approval_card.rs` | 内嵌审批卡（list 末项） |
| `input_area.rs` | Composer + model 菜单族 + workspace 确认锚点 |
| `inspector.rs` | Changes / Terminal / Resources 三页签 |
| `changes.rs` | Files / Summary 二级页签 + DiffView（hunk 语义着色 + `overflow_x_scroll`）+ ActivityPopover |
| `resources.rs` | MCP 只读列表 |
| `task_rail.rs` | 侧栏装配 + grouping/scope 菜单 + 长标题 `.truncate()` |
| `text_input.rs` | 单行 TextElement（IME marked_range、UTF-16 映射） |

`ui/components/` 组件库（7 模块 11 公开类型）：`Button`（variant Primary/Ghost/Danger/Success/Raised/Icon + hover/active）、`Dropdown`/`MenuPanel`/`MenuRow`（`deferred(anchored())` 浮层 + `occlude()` 滚轮无穿透）、`FollowScroll`/`BackToBottom`、`Label`/`Badge`、`ListRow`、`Panel`、`StatusBar`。五组菜单（grouping/scope/model/entry fork/workspace confirm）全部浮层化，`Option<MenuKind>` 单开互斥、Escape/外点关闭为宿主接线。

---

## 附录 A. 历史 K-03 取证记录（旧 R8 波 E，2026-08-24）

> 本附录保存旧路线的取证，不再承担当前签字。未完成项已全部移交 [新 R7–R8 任务书](../plan/R7-R8-ui-quality-gates.md)，且 [UI Review](UI_Review.md) 已撤回会降低 99% 目标的旧偏差接受。

验收环境：macOS 3440×1440 @1x；隔离实例 `r8e`（`PAWORK_DATA_DIR=/tmp/r8e.Xepxwh`），HEAD=8b0e3a0（波 D 收口）。

### A.1 自动化已取证项（主代理，截图实证）

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 真窗口启动 + Connected 壳层（Inspector 展开） | ✅ rail / 空 Timeline / Composer / Inspector 三页签 / 状态栏与基准一致 | /tmp/r8e-recon2.png |
| Inspector 折叠 + ActivityPopover | ✅ 折叠态 Workspace 扩展；popover「Changes · unavailable」诚实缺省，无伪造 0 | /tmp/r8e-popover.png |
| grouping 浮层菜单 | ✅ `✓ Timeline / Projects` 单开浮层、不占布局流 | /tmp/r8e-grouping.png |
| model 浮层菜单 | ✅ Connected 态锚点上翻浮层，列已配置模型 | /tmp/r8e-model2.png |
| 断线 fail-soft | ✅ 连接失败/断线如实上屏（传输因 socket refused 可见）+ Reconnect 入口；恢复路径走查见 A.2 | /tmp/r8e-full2.png |
| 1080×720 最小窗口 | ✅ 最小窗口布局完整不溢出（rail / Composer / 状态栏）；拍摄时为断线态（机制性 idle 关闭，见 A.3-D3），Connected 态最小窗可用性走查见 A.2 | /tmp/r8e-1080c.png |
| 菜单断线归一化 | ✅ 断线态 model 菜单不打开、触发器仅焦点环（model 翻假归一化） | /tmp/r8e-model.png |
| 心跳保活 soak（D3 修复后二进制，未提交 diff） | ✅ 空闲 >2min 仍 Connected（修复前 ~30s 必断）；lsof 每 60s 采样实证对等连接 33min+ 持续存活（20/20 peers=2，归档 /tmp/r8e-keepalive.log，21:30:56–21:49:57） | /tmp/r8e-soak.png（Connected 态） |

### A.2 历史未完成走查项（已移交新 R7–R8）

- [ ] IME：中文输入法组合中 Enter 不发送，候选窗位置正常（§6）
- [ ] 多行粘贴：粘贴多行文本保持原文、Shift+Enter 换行（§6）
- [ ] 1440×1024 逐屏对照 design/ 三图（Timeline / 折叠 / Projects）
- [ ] 纯键盘走查（基准 §3.6）：R7 Wave B 已自动通过 Tab 链、菜单 ↑/↓/Enter/Escape、task cycling / next-needs-attention、审批与焦点恢复；VoiceOver、IME 的系统级人工复核与 R8 完整汇总仍未执行
- [ ] 菜单三例：外点关闭后再点同触发器可重开；输入框聚焦时 Escape 关菜单不吞键；滚回底部重挂跟随时机
- [ ] Reconnect 恢复：R6 Wave B 已覆盖 Inspector/Terminal/Resources 与会话选择恢复；仍需 R8 汇总验证进行中 Run 的全应用生命周期
- [x] Connected 态 1080×720 最小窗：R7 Wave C 已以真实 Host / Desktop 覆盖 Connected、ActivityPopover、Disconnected 与三轮宽窄 resize；Composer / 状态栏 / Inspector 触发器可用。连接长文案以定宽槽截断显示省略号，截图级 paint 门禁与最终 U2 全绿（[证据](ui-review/r7-wave-c/notes.md)）。该结论只覆盖默认字号与当前平台偏好态。
- [ ] 虚拟化四例（长会话）：滚动流畅 / 回底重挂 / Entry「···」菜单锚点 / 长标题 truncate
- [ ] hover / active 交互态抽查（基准 §8.1 取值表）
- [x] DiffView 横滚：R6 Wave B 真窗口长行通过，AX 记录 horizontal offset `-720.0 / 2477.0`（[证据](ui-review/r6-wave-b/u2-reviewfix-pass-20260830/)）
- [x] 千级事件功能门禁：R7 Wave C 临时派生 1024 行真实投影，虚拟化、离底 / 回底与 CJK/emoji 末尾哨兵通过，并归档单次时延基线（[证据](ui-review/r7-wave-c/u2-reviewfix-pass-20260830/performance-baseline.json)）。
- [ ] 千级事件性能回退门禁：仍需重复干净机器样本、阈值与帧率观察；当前 `baseline_only` 不宣称无回退。

### A.3 漂移与定夺项

- D1 mod.rs 行数：824（波 C 达标 <900）→ 1031（波 D 三页签接线）——✅ 已拍板（2026-08-24）：接受 1031 为终态口径，不再重瘦。
- D2 窄窗响应式：1080–1279 时 rail 收敛 240px + Inspector 默认折叠未实现（固定 288px，V2 起既有）——历史上曾接受延期；2026-08-25 的 99% UI Review 已撤回该完成口径，现为 R7/R8 必过功能门禁。
- D3 空闲 30s 断连：host 30s 心跳超时 + desktop 无周期心跳的机制性关闭，非 R8 回归——✅ 已修复（2026-08-24）：desktop controller 泵循环连续 15s 空闲发 `heartbeat()`，真窗口 soak >2min 不再断连实证，desktop 测试 41/41 绿。
- D4 P3-4 Entry 菜单滚动卸载：菜单开着时条目滚出可视区被卸载、浮层消失但状态残留（滚回自现，Escape/外点仍有效）——✅ 已拍板接受（2026-08-24）：虚拟化卸载语义下浮层随条目回收属可接受行为。

### A.4 已收口免重复项

Changes 四面 / Resources 行 /「@」端到端 bubble 双 part 已于波 D 真窗口逐项截图实证（/tmp/r8d-*.png）；旧 R8 过程与 S12-CR09 复核已归档于 [history](history.md)。这些历史截图不替代新 R8 的固定 fixture、同尺寸差分和用户签字。
