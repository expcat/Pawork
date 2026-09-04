# Pawork Desktop GUI 设计

> Desktop GUI 的设计事实源。目标 UI 全面优化方案见 [gui-optimization.md](gui-optimization.md)，分阶段任务见 [gui-roadmap.md](gui-roadmap.md)；视觉实施基准见 [../design/README.md](../design/README.md)。产品/验收汇总见 [spec/desktop.md](spec/desktop.md)；包级 Spec 见 [spec/crates/desktop.md](spec/crates/desktop.md)。

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

空态：无会话时主区只有一句提示和 Composer。不以假卡片冒充未实现能力。

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

模型选择器只列 Host 返回的已配置条目；切换只影响下一轮，并以 Core 的确认事件覆盖本地 pending。审批按钮映射 `ApproveOnce / ApproveForRun / Deny`；关闭审批卡片不能等价于允许。

### 3.2 TaskRail

阶段视觉基准为 [P0 Foundation](../design/desktop-ui-p0-foundation-v4.png)、[P1 Run & Review](../design/desktop-ui-p1-run-review-v4.png) 与 [P2 Settings & Polish](../design/desktop-ui-p2-settings-v4.png)。

- 顶部 `All projects / <project>` 是范围筛选；`Timeline / Projects` 是分组方式。两者正交。
- 分组方式使用标题行右侧 28×28px 二态直接切换按钮，不保留宽幅 segmented control，也不打开下拉菜单。
- 当前为 Timeline 时显示 folder icon、tooltip / AX name 为 `Show projects`；当前为 Projects 时显示 clock icon、tooltip / AX name 为 `Show timeline`。图标表达目标动作，AX value 表达当前视图。
- click、Enter、Space 与 AX Press 立即切到另一种分组；切换后焦点留在按钮，active session、Composer 草稿、Run、scope 与项目展开状态不变。
- 全局 `AddTaskButton` 在连接行；每个项目头另有定向新建，绑定该项目 canonical `workspace_id`。断线与 stale 时两类入口均禁用。
- Timeline 层级：日期（Today / Yesterday / Previous 7 days / Earlier）→ 项目 → Task，均按最近活动倒序；Task 行不再重复项目名。
- Projects 按 canonical Workspace 分组；缺失元数据的 Session 进入 `Unassigned`。
- 切换分组不改变 active session、Composer 草稿、Run 或主 Timeline。
- 分组方式、范围筛选和项目展开状态是本地 presentation preference，不新增 domain 事件，也不改协议。

### 3.3 Context、运行信息与 Inspector

- Composer 常态高 88–94 px，同行控件高 28–30 px；模型 / reasoning 只在模型选择器显示。
- `ContextMeter`：当前请求上下文估算 / model catalog context window。容量未知时显示 unavailable，不用 Session 累计 token 冒充。
- Workspace 与 Inspector 底部共享 24 px `RunStatusBar`：Task 累计 token、Provider 剩余额度、output tokens/s 与 Run duration；缺权威来源时显示 unknown / `—`。
- Inspector 顶层：Changes / Terminal / Resources。折叠时宽度归零，右上 `ActivityPopover` 只摘要已有的 Changes 事实；Surface 未接通时隐藏对应分区，不做可点击假入口。
- 只消费 projection / Host capability，经 controller → `pawork-client`；GUI 不直连 Provider、quota、Git、PTY 或数据库。

### 3.4 可见层级

- Timeline 消息、tool 与 summary 占满可用宽度，以 618px 可读列封顶。同一 Run 的连续 tool 合并为一个 group，标题汇总数量与真实状态，默认展开并可由 click、Enter、Space、AX Press 折叠；折叠键取首个 tool event id，live 与 replay 结构一致。
- Run 终态只保留一个 summary；只有当前 Session 存在真实 Changes 时才显示 `Review changes`，并打开、聚焦 Changes。失败、取消与审批状态均使用文字 / 图标，不只依赖颜色。
- TaskRail 项目计数和任务时间使用 56px 右对齐 meta 槽；标题 `truncate`。
- Changes 文件行使用固定槽；DiffView 有只读路径 header 与增删 marker gutter；Changes / Resources 的 empty、error、stale 各自给出诚实说明。
- ActivityPopover 保持 320px 宽、按当前唯一 Changes 内容收缩为 144px 高、右上锚定并维持 capability honesty。

### 3.5 Settings

Settings 沿用深色主题、8px 节奏和 1440×1024 基线，不把工作台改造成 Dashboard：

```text
┌──────────────────┬────────────────────────────────────────────┐
│ Settings Rail    │  内容区                                    │
│ ← Back to workspace│                                           │
│ Models & providers│                                            │
│ General           │                                            │
│ Approvals         │                                            │
│ Tools & MCP       │                                            │
│ Terminal          │                                            │
│ Appearance        │                                            │
│ Advanced          │                                            │
│ About             │                                            │
└──────────────────┴────────────────────────────────────────────┘
```

- 入口位于 TaskRail 底部 `Local` 行右侧 gear。进入后左栏换成 Settings Rail；Timeline、Composer、Inspector 不渲染。
- `← Back to workspace` 恢复进入前的 session、Timeline 位置、Composer 草稿、Inspector 和 Run；Settings 不取消 Run。
- 导航与页内可见文案统一 English，顺序为 Models & providers → General → Approvals → Tools & MCP → Terminal → Appearance → Advanced → About。没有真实读写能力的页不显示；Advanced 离线仍可进入。
- **Models & providers**：内容最大宽 820px；provider 使用 64px 概览行，分列显示认证方式、连接状态与目录 / 模型数。Host `provider_auth_status` 是权威数据，Desktop 不按供应商名称硬编码 OAuth/API key 分支。普通行与 AX summary 不显示 masked credential、endpoint、catalog error 或 raw model id；endpoint / 错误只在连接、等待或删除确认详情出现。API key editor 仅在 Connect / Replace 后展开，secure input 的完整值不得进 AX tree、日志或状态文本。OAuth 只显示授权 URL、device code、到期/取消，不接触 token。认证成功与目录成功是两个状态；默认模型使用独立 section。
- **General**：Global `proxy_url`；未设置显示 `Not set (uses system environment variables)`；新 OAuth / 验证 / 目录同会话生效，当前供应商模型流量于切换或重启后生效。
- **权限与审批**：五档审批模式使用整行 radio，row click、Enter、Space 与 AX Press 同一 handler；会话信任开关、Global 默认只读行不变。变更仅当前会话生效、不持久化、进行中 Run 不受影响。
- **Tools & MCP**：复用 Host `mcp_list`，提供 Test / Remove。
- **Terminal**：Global `[terminal]`（shell / columns / rows）；只影响之后创建的终端。
- **Appearance**：Desktop 本地；三档 100%/125%/150% 与 `Cmd+=` / `Cmd+-` / `Cmd+0` 共用 `TextScale`，并显示随选择即时变化的正文 / control 字阶样例；仅当前 Desktop 会话生效，重启恢复 100%。主题只读深色，不画 light/system 控件。
- **Advanced**：本地连接诊断；断线仍可达。只读 runtime ID、协商 API、capabilities、endpoint、resume/ack，以 definition list 呈现。不展示 GUI token，不从 endpoint 反推 data directory。
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
- Composer 保持紧凑；ContextMeter 与 RunStatusBar 必须区分当前上下文、Session 累计 usage、quota、tokens/s 与 Run duration。缺值诚实显示 unavailable。
- 工具调用是 Timeline 里的折叠块，不是单独 IDE 面板。
- 流式输出按 token/事件追加；取消只取消当轮，历史保留。
- 审批 fail-closed：无用户动作不得当默认允许。
- 主题当前仅 dark 基线，不读取系统显示偏好，不构成第二套主题。
- `Enter` 仅在 IME 未组合时发送，`Shift+Enter` 换行；多行粘贴保持原文。
- Timeline 只在用户位于底部时追随流式输出；用户向上阅读后不得抢滚动位置。向上脱钩的滚动区提供回底控件。
- 连接、Run、tool 与审批状态必须有文本/图标语义，不能只靠颜色；主路径可全键盘操作。
- Accessibility 以 Desktop 显式语义树为唯一来源：稳定 identifier 与可本地化 label 分离；macOS 由 AppKit bridge 导出。应用内字号 `Cmd+=` / `Cmd++`、`Cmd+-`、`Cmd+0` 在 100%/125%/150% 间切换。新增可见交互必须同批补语义。当前 UI 无动画，Reduce Motion 无渲染分支。
- 可交互控件必须有 hover 与按下态；hover / active 只改背景，不引起布局移动。
- scope、model、entry、Activity 菜单为 `anchored()/deferred()` 浮层，同一时刻单开互斥；选择 / 再点触发器 / `Escape` / 点击浮层外关闭；打开时滚轮不穿透。Timeline / Projects 直接切换按钮不属于菜单。
- 用户发起的 task 切换、审批决策后焦点回到 Composer；session reset 先关闭旧菜单。
- Timeline 条目经变高虚拟化；侧栏长标题单行省略号截断。
- Resources 只读呈现 MCP 状态，字段缺失显示 unknown；无 Host 出口的分区不画入口。
- `@file` 由 Host 在 run_start 展开为独立 Text part，客户端不本地拼文件内容。

响应式：`1080 × 720` 为功能门禁。100% 字号时 rail 收敛 240px、Inspector 默认折叠、中央对话区 ≥560px；150% 时 rail 320px，窗口不足 1320 时保持 Inspector 折叠。

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
| `changes.rs` | Files / Summary + DiffView + ActivityPopover |
| `resources.rs` | MCP 只读列表 |
| `task_rail.rs` | 侧栏 + grouping 直接切换 + scope 菜单 |
| `text_input.rs` | 单行 TextElement（IME、UTF-16 映射） |
| `settings/` | Settings Rail 与各页 |

`ui/components/`：`Button`、`Dropdown`/`MenuPanel`/`MenuRow`、`FollowScroll`/`BackToBottom`、`Label`/`Badge`、`ListRow`、`Panel`、`StatusBar`。scope / model / entry / Activity 菜单全部浮层化，`Option<MenuKind>` 单开互斥；grouping 使用普通 Button 直接切换。
