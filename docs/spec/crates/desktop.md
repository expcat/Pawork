# pawork-desktop（apps/desktop，二进制）

> 本机单窗口 GPUI Agent 壳（TaskRail + Timeline + Composer + Inspector）：独立进程经 GUI Connection Protocol 连接 `pawork gui serve`，业务依赖**仅** [pawork-client](client.md)；依赖方向为 desktop → client →（re-export）protocol/transport 类型，不被任何包依赖。

## 1. 职责与边界

- **产品定位**：Pawork 桌面工作台。渲染会话列表、时间线、审批卡、变更 / 终端 / MCP 资源面板，把用户操作转成 GUI Connection Protocol 的 Command / Query，把 host 事件流投影成可渲染状态。
- **架构红线**（见 [../../architecture.md](../../architecture.md)）：不嵌入 Core；不直连 Provider、数据库、工具、Git、PTY、quota store；一切能力经 CLI 宿主（`pawork gui serve`）代理。
- **四层结构**（`main.rs` 的模块声明即分层，越界 import 视为违规）：
  - `ui/`：GPUI 渲染与交互。`AppView` 宿主 + 按 Surface 拆分的域模块 + `ui/components/` 基础组件库。
  - `projection.rs`：纯状态机，**不** import gpui / tokio / OS API；时间线条目语义委托 client re-export 的 `pawork_client::projection` 共享 reducer。
  - `controller.rs`：唯一业务出口是 `pawork-client`；所有 client 调用跑在 tokio runtime 上，结果经 `smol::channel` 投回 UI 线程。
  - `platform.rs`：socket / token 路径发现 + tokio Runtime 宿主；不触碰 GUI 与业务协议。
- **依赖 deny-list**：生产 `pawork-*` 依赖恰好 `{pawork-client}`。由 `platform.rs` 内测试 `desktop_production_pawork_deps_stay_client_only` 解析本包 `Cargo.toml` 断言，扫描器覆盖 `[dependencies]`、`[target.'cfg(...)'.dependencies]`、`[dependencies.<alias>]` 与 `package = "..."` 重命名形态；dev-dependencies 不计入。
- **能力面**：握手宣告 `Events` / `Snapshots` / `Approvals` / `TerminalStreaming` 四项；**不**宣告 `ArtifactStreaming`（K-08）。
- **断线语义**：断线不取消进行中的 Run（ADR-026）；主动 `disconnect` 只关闭连接，不发 RunCancel。

## 2. 模块与文件地图

29 个 `.rs` 文件、约 13.1k 行，全部在 `[[bin]] pawork-desktop` target 内（无 lib target、无 crate `tests/` 目录）。

| 路径 | 行数 | 承载内容 |
| --- | --- | --- |
| `src/main.rs` | ~670 | 入口与手动 argv 解析（非 clap）；`PAWORK_UI_BARRIER_DIR` env 读取（空值视同未设置，None 全程零开销）；`run_app`（1440×1024 居中窗口 + `WINDOW_MIN_SIZE` 1080×720 最小尺寸——R2 Wave B 设计响应式底线，再窄击穿 Workspace ≥560 合同——+ 沉浸式 titlebar：TitlebarOptions appears_transparent，traffic lights 悬浮深色壳、内容视口贯通全窗，R2 Wave A；`install_keybindings`、聚焦 Composer、安装 macOS AX bridge）；`run_probe` / `run_probe_smoke` 无窗冒烟模式及其 `wait_for_*` 事件等待器；1 个测试 |
| `src/controller.rs` | ~1470 | `DesktopController`（connect / 事件泵 / 空闲心跳 / 全部 Command·Query 构造与响应解析）；`ControllerEvent` 枚举；`DiffFileSummary` / `DiffFileDetail` / `DiffHunkDetail` / `DiffLineDetail` / `GitDiffInfo` / `McpServerEntry` 视图模型；11 个测试 |
| `src/platform.rs` | ~230 | `Platform`（tokio multi_thread Runtime，`handle()` / `block_on()`）；`default_socket_path` / `socket_path_for_instance` / `token_path_for_instance` / `token_path_for_socket` 路径发现；deny-list 断言；4 个测试 |
| `src/projection.rs` | ~2640 | `DesktopProjection` 渲染适配投影；`ConnectionState` / `ResumeState` / `ResumeApply` / `TerminalState` / `PendingApproval` / `ModelEntry` / `ActiveRun` / `SessionSummary` / `WorkspaceSummary` / `SessionLiveStatus`（R3 Wave A：Running / NeedsInput；R3 Wave B 增 Blocked——最近一条 `RunChanged` 为 failed / interrupted 终态 live 派生，优先级 NeedsInput > Running > Blocked；`session_live_status()` 跨会话读 active_runs 成员关系，apply_event 在 active-session 闸门前维护 live `RunChanged` 成员登记与后台 `ToolApprovalRequired`/`ToolCompleted`；`note_session_run` 在 `MessageSent` 乐观登记 Running；`session_unread()` 独立 unread 通道——非 active session 的 Session-stream 活动事件标记、`select_session` 清除）/ TaskRail 分组类型；snapshot 段解析器；F-13 定稿 `run_status_label`（Task tokens | quota | tok/s | Run 竖线分隔）与 `show_reconnect` / `workspace_empty_hint_visible` 可见性谓词（render 与 AX 共源）；26 个测试 |
| `src/ui/mod.rs` | ~2060 | `AppView` 宿主：连接生命周期、`ControllerEvent` 消费（含分页进行中 / 事件静默跟踪）、`MenuKind` 单开互斥与外点衔接标记、gpui 动作与键位表 `APP_VIEW_KEYBINDINGS`（R3 Wave B 增 cmd-alt-up / down / n 任务循环与 next-needs-attention）、tab_stop 表 `MAIN_PATH_TAB_STOP_IDS` 与 rail 前缀 `RAIL_TAB_STOP_IDS`（scope → grouping → 全局新建，行级 -17 档、composer 1 档链尾；`RailStop` 焦点链按 design §3.6 组装项目头 / 定向新建 / task 行，折叠项目只保留头部）、Tab / Shift-Tab 映射 `focus_next` / `focus_prev` 走真实 tab_index 链（Slice 4；GPUI 无默认 tab cycle，macOS 裸 Tab 被 NSWindow 在 keyDown 前吞掉，主机制为 `install_appkit_tab_monitor` 的 NSEvent 本地监听器——Apple block ABI `BLOCK_IS_GLOBAL=1<<28`（Slice 5 修正，原误用 1<<30 即 BLOCK_HAS_SIGNATURE），根节点 on_key_down 分支保留作后备；旧 `setAllowsKeyboardNavigation:` 已被现代 macOS 移除不可用）、菜单键盘高亮（Slice 5 起 Grouping / Scope / Model 菜单打开即接管 ↑/↓/Enter/Escape，不再以触发器聚焦硬门控——Tab 移焦 / 外点后键盘仍归菜单；Esc 关闭并焦点回触发器；`pending_keyboard_menu_select` 吞 Enter 后触发器 keyup 合成 click 防重开；行级 `pending_row_key_activate` / 按钮级 `pending_button_key_activate` 同构吞行 / 按钮键盘激活后的合成 click，Slice 5 起标记不匹配同样吞除防跨行误触发）、任务循环 `cycle_active_task`（按当前 rail 可见序，已处目标态短路不重开会话，空列表 no-op）与 `open_next_needs_attention`（NeedsInput > Blocked > Unread，active 之后循环起算，无候选 status_hint 如实提示）、`rail_scroll_to_active` 标记驱动 grouping/scope 切换后滚动 active task 到可见、`pending_scope_focus` 标记驱动 SnapshotRequired/Fresh 后 active 消失时焦点回退 scope 触发器、空态引导文案 `WORKSPACE_EMPTY_HINT`（视觉与 AX 共源）、三栏整体装配（经 `shell_layout::resolve` 响应式决定 rail 宽与 Inspector 折叠）、StatusBar（F-13 信息串居中 + 右侧 Inspector 触发器）与 AX 同步；1s tick（run 时钟重绘 + barrier settle 发射）；9 个测试 |
| `src/ui/shell_layout.rs` | ~240 | R2 Wave A 壳层几何合同：`resolve`（唯一计算入口，render 与 AX 树共享）——宽窗 rail=288 / Inspector 440，窗口宽 ≤1279 时 rail=240 且 Inspector 强制折叠为抽屉（Workspace ≥560 底线）；rail 顶部 36px traffic-light 安全区占位；4 个测试（阈值切换 / 1440 合同 / 1080 折叠 / resize 恢复） |
| `src/ui/accessibility.rs` | ~410 | 平台无关 `AxTree` / `AxNode` / role / action / request / rect 模型，identifier 唯一性、层级、focus / hit-test 约束；非 macOS no-op facade；3 个测试 |
| `src/ui/accessibility/app.rs` | ~1480 | 从 `AppView` canonical UI 状态与布局 metric 构建三栏语义树；壳层几何与 render 共享 `shell_layout::resolve`（含 36px traffic-light 安全区与窄窗 Inspector 折叠）；TaskRail 几何与 render 共享 `metrics::RAIL_*`（R3 Wave A），连接行文案同源 `connection_status_label`、会话行状态词同源 `session_status_description(status, unread)`（R3 Wave B 增 Blocked 状态词与「· Unread」unread 语义，与状态点 / 标题 semibold 视觉同源）；rail 项目头 / 定向新建 / 会话行与菜单高亮行补 focused 标志（查 `rail_row_focus` 句柄）；空态引导只读节点 `workspace-empty-hint`（与 timeline_area 可见条件同源，无 action）；run-status AX frame 随 F-13 居中；TaskRail 镜像 grouping/collapse（日期组 → 项目头/新建 → 会话行，折叠只投影头部）；稳定动态 identifier（Timeline 桶限定防重）；Inspector 折叠态走 ActivityPopover 链路；AX Send 复用 IME composing 闸门；把 press / focus / set-value 白名单映射回既有 AppView handler 与 enable gate；4 个测试 |
| `src/ui/accessibility/macos.rs` | ~940 | ADR-042 AppKit bridge：`GPUIView` AX root、`NSAccessibilityElement` 虚拟元素、frame / parent / hit-test / focus / notification / retain-release、settable/action 双门与 action 回调；结构不变（identifier/role/press 能力/子树形状）时原位刷新既有 element 而非整树重建，内部树同步 super 直调不触发 action 回调；6 个 macOS 测试 |
| `src/ui/barriers.rs` | ~175 | UI fixture barrier 发射器（R1 Wave B）：`BarrierSink` 读 `PAWORK_UI_BARRIER_DIR`（None 零开销直通）；`timeline_stable`（settle_seq 单调自增 / session_id / entry_count）重写与 `approval_visible` 写/删；tmp+rename 原子替换、IO 失败静默；1 个测试 |
| `src/ui/theme.rs` | ~380 | 深色单主题 token：六组 29 色（bg 3 / surface 3 / border 2 / text 9 / accent 3 / semantic 9）+ 字阶 `font`（XS=11 / SM=12 / BASE=13 / MONO="Menlo"，R3 Wave A 增 TITLE=22 / BODY=18 / BODY_SM=17）+ `metrics` 31 个尺寸常量（含 R3 Wave A 新增 `RAIL_*` 15 个 TaskRail 几何：INSET=20 / ICON_BUTTON=28 / DOT=10 / 行高 36·44 / 各段间距，render 与 AX 共享；同时退役仅 rail 消费的 ICON_SMALL / ICON_MEDIUM / ICON_LARGE）；静态 `dark()` 访问器；`impl Global` 仅为未来主题挂载点。R2 Wave A 按 design/README.md §2.1 重定色板（含 text.assistant→emphasis、text.tool→secondary 收敛、placeholder 不透明 #7f7f7f、新增 semantic.success_fg #74c94c）；5 个定向测试 |
| `src/ui/timeline.rs` | ~165 | Timeline 容器：gpui `list()` 变高虚拟化 + `ListAlignment::Bottom` 钉底；空态引导（无 active session 且条目数为 0 时居中 tertiary 一句，R2 Wave B）；`install_scroll_follow`（脱钩检测）；`sync_list`（统一 reset、脱钩偏移恢复、Entry 菜单 close-on-reset）；`TIMELINE_OVERDRAW`=200px；回底控件接线 |
| `src/ui/timeline_entry.rs` | ~140 | `timeline_entry_element`：五类条目渲染 + 条目「···」fork 菜单（MenuPanel/MenuRow）；`on_fork` 入口级复核 |
| `src/ui/approval_card.rs` | ~110 | 审批卡：警示卡 + Allow once / Allow for run / Deny 三按钮；app 级 focus handle（虚拟化卸载不丢失）；禁用原因 tooltip |
| `src/ui/input_area.rs` | ~330 | Composer：model 菜单（MenuRow 接 R3 Wave B 键盘高亮，selected 同源 `effective_model`）、ContextMeter、workspace 标签、WorkspaceConfirm 浮层（无触发器、锚 label 行下方）、输入行 + Cancel / Send、状态提示行与全部禁用原因文案 |
| `src/ui/inspector.rs` | ~265 | Inspector 面板：顶层 Changes / Terminal / Resources 三页签（`InspectorTab`，默认 Terminal）；Terminal 页（cwd / size 行、FollowScroll 输出区 + 回底、终端输入 + Start/Size）；无输出占位文案 `TERMINAL_EMPTY_OUTPUT`（视觉与 AX 共源，R2 Wave B）；`ensure_terminal` 懒创建；1 个测试 |
| `src/ui/changes.rs` | ~705 | Changes 面：Files / Summary 二级页签、`ChangesPanelState`（双 epoch 防过期）、文件清单、DiffView（hunk 着色 + 横滚）、session_mismatch banner、ActivityPopover；6 个测试 |
| `src/ui/resources.rs` | ~210 | Resources 页：MCP server 只读表 + `ResourcesPanelState`（epoch 防过期）+ 手动刷新；1 个测试 |
| `src/ui/task_rail.rs` | ~775 | Sessions 侧栏（F-03/F-04 定稿，R3 Wave A；R3 Wave B 增键盘导航与状态扩展）：顶部三行——标题行「Pawork」22px + ghost grouping 角标 28×28、全宽 raised scope 行（h36 / 1px 描边 / r4 / 18px 左对齐）、连接行（Ø10 状态点 + `connection_status_label` 17px + 28×28 全局「+」）；grouping / scope 菜单（MenuRow 键盘高亮）、日期桶头（18 medium secondary）与项目块（chevron + 名称 + 右对齐计数 + 28×28 定向「+」）折叠、44px 任务行（状态点 + 标题 `.truncate()` + 17px 相对时间右对齐，选中 raised + r4；Blocked=danger 红实心、unread 标题 SEMIBOLD 同字号不改几何）、Reconnect 按钮（仅 Disconnected / ConnectFailed，Connecting 不显示，R2 Wave B）、连接状态徽标、「Local」页脚（honest-hidden，TR-12）；桶头 / 项目头 / 任务行平铺为滚动容器直接子元素（`ScrollHandle::scroll_to_item` 拿行级 bounds，grouping/scope 切换后把 active task 滚到可见，折叠项目退回头部行）；rail 内键盘导航 `handle_rail_navigation_key`（↑/↓ 沿焦点链移焦 clamp 不 wrap，项目头 ←/→ 收起/展开已处目标态 no-op，Enter/Space 经 ListRow 行级 key_down 直接调激活 handler（Slice 4）；Slice 5 菜单打开时 rail 让位不接管；rail 聚焦 Button——grouping/scope/add-task/项目定向「+」——裸 Enter/Space 行级激活同 click 路径，菜单已开时让位给根节点菜单 Enter 接管），带修饰键不接管 |
| `src/ui/text_input.rs` | ~730 | `TextInput`（Composer / 终端共用）：内容 / 占位符 / IME `marked_range` 下划线 / UTF-16↔UTF-8 映射（`EntityInputHandler`）/ 多行高度钳制 / 光标与选区绘制 / 点击聚焦；AX set-value 使用同一 `set_text` 入口并清 marked range；2 个测试 |
| `src/ui/u1_probe.rs` | ~300 | R1 Wave C U1 spike（`#[cfg(test)]`）：真实 `TextInput`/`Button`/overflow 滚动容器上的 TestAppContext 进程内驱动探针（action/focus/key/mouse/scroll/resize/clipboard/确定性 executor/debug_bounds）；10 个测试 |
| `src/ui/components/mod.rs` | ~10 | 组件族模块声明 |
| `src/ui/components/button.rs` | ~340 | `Button`：五 variant（Primary / Ghost / Danger / Success / Raised；Icon 于 R3 Wave A 退役，无消费点）的底色·文字色·hover/active 映射；`ButtonPadding` 四档；`bordered()` / `radius()` / `center()` / `vcenter()` 描边与对齐修饰（R3 Wave A）；tab_stop + track_focus + 聚焦描边三件套；tooltip；`on_activate` 行级键盘激活（Slice 5 P2b：聚焦按钮裸 Enter/Space 直接调激活 handler 禁合成 click；disabled 不激活；是否 stop 由调用方决定） |
| `src/ui/components/dropdown.rs` | ~210 | `Dropdown`（触发器 + `deferred(anchored())` 浮层）、`MenuPanel`（`occlude()` + `on_mouse_down_out` + `MENU_MAX_HEIGHT`=240px 内滚动）、`MenuRow`（选中 / 禁用 / hover 语义；R3 Wave B 增 `highlighted` 键盘高亮——未选中高亮行 surface.raised 与 hover 同 token，选中行保持 accent.primary 不叠加）、`ANCHOR_GAP_Y`=4px |
| `src/ui/components/follow_scroll.rs` | ~90 | `FollowScroll`（`ScrollHandle` + 跟随位：贴底判定 / 脱钩 / 重挂；现仅终端使用）与 `BackToBottom` 回底控件容器（绝对定位右下） |
| `src/ui/components/label.rs` | ~70 | `Label`（单行文本，token 化字号 / 颜色）与 `Badge`（状态徽标别名，默认 XS + text.secondary） |
| `src/ui/components/list_row.rs` | ~130 | `ListRow`：Task 行（选中态底色，水平 `px_2`）与 ProjectHeader 行两形态，行高 44 + 垂直居中（R3 Wave A）；`min_w_0` 保证子项 truncate 拿到确定宽度；R3 Wave B 增 `track_focus` 键盘焦点三件套（tab_stop + track_focus + 聚焦 accent 描边，同 Button 模式）与 `on_activate` 行级键盘激活（裸 Enter / Space key_down 直接调与 click 同一激活 handler，Slice 4；禁合成 click 兜底；Slice 5 起 stop_propagation 由调用方 handler 决定——菜单打开时让位给根节点菜单 Enter 接管） |
| `src/ui/components/panel.rs` | ~80 | `Panel`：`side_right`（TaskRail，右描边 + gap/p-2）与 `side_left`（Inspector，左描边）固定宽面板壳 |
| `src/ui/components/status_bar.rs` | ~80 | `StatusBar`：底部 24px 状态行容器（顶描边 + XS 次要文字）；F-13 布局——信息串 `centered()` 行内绝对居中（不受右侧触发器宽度偏移），流式子元素靠右 |

## 3. 用户可见界面与交互面

### 3.1 启动参数与运行模式

```text
pawork-desktop [--socket <path>] [--instance <name>] [--probe|--probe-smoke]
```

- 手动 argv 解析；未知参数打印 usage 并 exit 2。
- socket 解析：`--socket` 直接指定；否则按 `--instance` 推导 `<data_dir>/pawork-gui[-{instance}].sock`（`default` 等价无后缀）。
- `--probe`：不开窗，connect + snapshot + `model_list` 后打印一行 `connected: instance=… sessions=… models=… catalog=…` 退出（成功 0 / 失败 1）。
- `--probe-smoke`：同一条 controller 路径跑真实冒烟——流式回合、切模型、写文件触发审批、取消 run、两次断线重连（持久化回放 + `disconnect_survive` 断言进行中 run 未被断线取消），打印签名行退出。
- 正常模式：1440×1024 居中窗口、最小 1080×720（透明 titlebar，traffic lights 悬浮于壳层，rail 顶部留 36px 安全区），启动即聚焦 Composer。

### 3.2 三栏工作台（宽度：侧栏 288 / Inspector 440 / 状态栏高 24；窗口宽 ≤1279 时侧栏 240 且 Inspector 默认折叠为抽屉，Workspace ≥560）

- **TaskRail（左侧栏）**
  - 顶部三行（F-03，R3 Wave A）：标题行「Pawork」（22px semibold）+ ghost grouping 角标 28×28（Timeline ◷ / Projects ▤）；全宽 raised scope 行（h36 / 1px 描边 / 圆角 4 / 18px，All projects / 各 workspace）；连接行 Ø10 状态点 + `Local · Connected[ · {resume 文案}]` / `Connecting…` / `Disconnected · {reason}` / `Connect failed · {reason}`（17px secondary，文字态不只靠颜色）+ 28×28 全局「+」。Reconnect 主按钮仅在 Disconnected / ConnectFailed 相位出现，Connecting 进行中不显示。
  - Timeline 分组 = 日期桶（Today / Yesterday / Previous 7 days / Earlier）→ 项目 → 任务；Projects 分组按 canonical workspace，缺 `workspace_id` 进 Unassigned（无「+」）。
  - 日期桶头 18px medium secondary；项目头可点折叠（chevron ▾/▸ + 名称 + 独立右对齐任务计数）；项目级 28×28「+」按该 workspace 新建任务。
  - 任务行（44px，选中 raised + 圆角 4）：状态点 Ø10（Needs input=琥珀实心 = 有待审批；Running=accent 蓝实心 = 该 session 有 active run；Blocked=danger 红实心 = R3 Wave B live 派生的 failed / interrupted 终态，优先级 Needs input > Running > Blocked；其余空心灰圆不声明语义；wire 无每会话终态字段故不画终态绿点）、标题单行 `.truncate()`（unread 时 SEMIBOLD，同字号同行高不改几何，不加 dot / 徽标）、相对时间 17px 右对齐（now / Nm / Nh / Nd）；点击打开会话并聚焦 Composer。
  - grouping / scope 切换不改 active session 也不动分组展开状态，下一次 render 把 active task 滚动到可见（项目折叠时退回头部行；active 被 scope 过滤则诚实跳过）。
  - 底部账户区（F-04，TR-12 honest-hidden）：只保留「Local」本机身份行；头像 / 姓名 / quota / 组织等无权威来源元素一律不画。
- **Timeline（中栏上）**：虚拟化列表渲染五类条目——`You:` 用户消息、`Assistant:` 助手消息（流式增量合并为一条）、工具调用（`name · status` + 可选输出摘要）、运行状态行、`Error:` 错误行。空态（无 active session 且条目数为 0）居中一句 tertiary 引导「Select a task from the rail, or press Cmd+N to start a new one. Cycle tasks with Cmd+Opt+↓ / Cmd+Opt+↑, or jump to the next task that needs attention with Cmd+Opt+N.」（AX 同步 `workspace-empty-hint` 只读节点；Disconnected 保留旧条目时不显示）。每条右侧「···」菜单含 Fork（仅 reducer 判定的闭合 run 边界可用；不可用时灰字禁用行）。用户上滚脱钩后右下浮出「↓ 回到底部」。
- **审批卡**：`pending_approval` 存在时作为 timeline 末项渲染——警示底色卡片（`Approval · {tool}` / reason / 可选 preview detail）+ 三按钮 Allow once（Cmd+1 / Cmd+Return，Primary）、Allow for run（Cmd+2，Success）、Deny（Cmd+3，Danger）；断线时禁用且 tooltip 给出原因。
- **Composer（中栏下）**
  - 上行：model 菜单（`provider / display_name`；run 进行中、目录未加载或断线时禁用并 tooltip 给原因）、ContextMeter（`Context · — / {window}` 或 `Context · unavailable`）、workspace 标签。
  - 输入行：多行输入框（88–220px 随行数自适应）+ Cancel（Cmd+.，Danger）+ Send（Enter，Primary）。
  - 提示行：轮换显示 `status_hint` 或禁用原因（如「Run in progress — sending and model switch are disabled. Cancel remains available.」「Enter to send · Shift+Enter for newline」）。
  - All projects 范围下新建任务先弹 WorkspaceConfirm 浮层选定 workspace（`resolve_new_task_workspace` 判定）。
- **Inspector（右栏，cmd-i 开合）**：顶层三页签，默认 Terminal，各页滚动状态独立保留。
  - Changes：Files / Summary 二级页签 + ↻ 手动刷新。Files = 文件清单（路径 · status · `+A/−D`，≤200px 内滚动）+ DiffView（等宽 Menlo；hunk 头 raised 底 secondary 字；addition 行 success_bg 底 / deletion 行 danger_bg 底 / context 行 panel 底；长行 `overflow_x_scroll` 横滚不折行；binary 显示「Binary file — not rendered.」）。Summary = 七字段行（Session / Files / Lines / By status / Branch / Dirty files / Work dir，缺失显 unknown）。数据会话 ≠ 查看会话时顶端 banner 如实标注。
  - Terminal：host 流式 `TerminalOutput` 滚动文本（非 VT100、无本地 PTY）+ cwd 与 `列×行` 尺寸行 + 终端输入（Enter 写入，未启动时先懒创建）+ Start / Size 按钮 + 脱钩回底控件。
  - Resources：MCP server 只读表（name + state 徽标，`failed` 红字；`transport · N tools[ · last_error]` 次行）+ ↻ 刷新。
- **StatusBar + ActivityPopover**：底部状态行居中 RunStatusBar 徽标 `Task — tokens | Quota unavailable | — tok/s | Run {mm:ss|—|idle}`（F-13 定稿语序与竖线分隔；缺权威来源一律 `—`，不伪造）；Inspector 触发器保留最右（F-12 迁移 Workspace Header 后再移除）——展开态显示「Hide inspector」直接折叠，折叠态点击弹 ActivityPopover（320px：「Changes」标题 + 摘要行 `N file(s) · +A/−D` 或 `unavailable`；点击摘要展开 Inspector 并定位 Changes 页；必要时附 latest 会话提示行）。

### 3.3 键盘与焦点

| 上下文 | 键 | 动作 |
| --- | --- | --- |
| TextInput | enter | SendMessage（冒泡到 AppView 裁决） |
| TextInput | shift-enter | NewLine |
| TextInput | backspace / delete / left / right / home / end | 对应编辑动作 |
| TextInput | cmd-v / ctrl-v | Paste（`\r\n` 归一为 `\n`） |
| AppView | cmd-. | CancelRun |
| AppView | cmd-enter / cmd-1 | ApproveOnce |
| AppView | cmd-2 | ApproveForRun |
| AppView | cmd-3 | Deny |
| AppView | cmd-n | NewTask |
| AppView | cmd-i | ToggleInspector |
| AppView | cmd-alt-up / cmd-alt-down | TaskCycleUp / TaskCycleDown——按当前 rail 可见顺序循环切换 active task（空列表 no-op） |
| AppView | cmd-alt-n | NextNeedsAttention——按 rail 顺序找下一个 NeedsInput > Blocked > Unread 会话并打开；无候选 status_hint 提示「No task needs attention.」 |
| Grouping / Scope / Model 菜单（打开时，不要求触发器聚焦） | up / down | 移动键盘高亮（wrap；未移动时从当前选中项起算，菜单关闭复位） |
| Grouping / Scope / Model 菜单（打开时，不要求触发器聚焦） | enter | 选择高亮行（等价点击对应 MenuRow；触发器 keyup 合成 click 由衔接标记吞掉防重开） |
| TaskRail 列表 | up / down | 焦点沿 §3.6 停靠链步进（clamp 到两端，不 wrap；带修饰键不接管） |
| TaskRail 列表 | enter / space | 激活聚焦行：ListRow 行级 key_down 直接调用与 click 同一激活 handler（打开 task / 展开收起项目；R3 Wave B Slice 4 修复——GPUI keyup 合成 keyboard click 在真窗口不可达，不以合成 click 兜底；激活后的同键 keyup 合成 click 由 `pending_row_key_activate` 衔接标记吞掉防双触发——Slice 5 起标记不匹配同样吞除防跨行误触发；Grouping/Scope/Model 菜单打开时行级让位，Enter 由根节点菜单接管） |
| TaskRail 项目头聚焦时 | left / right | 收起 / 展开（已处目标态 no-op） |
| 根节点 | tab / shift-tab | focus_next / focus_prev——沿 tab_index 档位链走焦（GPUI 无默认 tab cycle；macOS 上 NSWindow 在 sendEvent 层吞掉裸 Tab，keyDown 路径收不到，主机制是 `install_appkit_tab_monitor` 的 NSEvent 本地监听器在派发前截获并调 `window.focus_next()` / `focus_prev()`，根节点 on_key_down 的 Tab 分支保留作非 macOS / 监听器失效后备；带 cmd/ctrl/alt 的组合键放行不接管） |
| 根节点 | escape | Grouping / Scope / Model 菜单打开时关闭并把焦点送回触发器（不要求触发器聚焦）；其余情况关闭当前浮层菜单 |

主路径按钮（`MAIN_PATH_TAB_STOP_IDS` 七项：approve-once / approve-for-run / approve-deny / cancel / send / add-task / model-picker）挂 tab_stop + track_focus + 聚焦描边三件套。

rail 聚焦 Button（grouping / scope / add-task / 项目定向「+」）上裸 Enter / Space 为行级键盘激活（Slice 5 P2b）：key_down 直接调用与 click 同一激活 handler（开菜单 / 新建），禁合成 click 兜底；激活后的同键 keyup 合成 click 由 `pending_button_key_activate` 衔接标记吞掉（防「刚开的菜单被闪关」与「重复新建」）；Grouping / Scope / Model 菜单已开时按钮让位，Enter 由根节点菜单接管；disabled 不激活。

Tab 焦点顺序（design §3.6，R3 Wave B）：rail 前缀三档 `RAIL_TAB_STOP_IDS`（-20/-19/-18：project-scope → task-rail-grouping → add-task）→ rail 行级 -17 档（项目头 / 定向 ProjectAddTaskButton / task 行，按当前分组渲染序；折叠项目只保留头部）→ 主路径 `MAIN_PATH_TAB_STOP_IDS` 0 档 → composer `COMPOSER_TAB_INDEX` 1 档链尾（wrap 回 rail 首停）。Tab / Shift-Tab 经 AppKit 本地监听器（后备：根节点 on_key_down）映射 `window.focus_next()` / `focus_prev()` 真实可走（Slice 4）；菜单键盘导航在 Grouping / Scope / Model 菜单打开时即接管（Slice 5 起不再要求触发器聚焦；rail 与行级 / 按钮级激活在菜单打开时让位，冒泡到根节点裁决），Escape 关闭一律回焦触发器。

## 4. 核心行为与数据流

### 4.1 启动 → 连接 → snapshot → 分页 timeline → live 事件 → 断线 Reconnect

1. `AppView::new` 即 `start_connect`；`DesktopController::connect` 先按 socket 文件名推导 token 路径（`pawork-gui-X.sock` → `gui-X.token`）并读 `gui.token`——缺失 / 不可读 / 为空即整个连接失败（fail-closed）。
2. 建 512 容量 `smol::channel`；`LocalTransport` + `ConnectOptions{ timeout 10s, client_label "pawork-desktop", 帧上限 1MiB }`；带上内存中的 last_acked（若有）。
3. **在 `runtime.spawn` 内**执行 `GuiClient::connect_with_resume_config`（含握手与能力宣告）→ 取 `initial_snapshot` → 按 resume 三态 ack：首连记录并 ack `snapshot_sequence`；`Replay` 记录并 ack `through_sequence`；`UpToDate` 只记录 `current_sequence`；`SnapshotRequired` 换用 `outcome.snapshot`（无则回退握手首帧）再记录并 ack → `subscribe_all`。连接期任何 client 调用都不得落在 gpui 前台执行器上（见 §8 崩溃教训）。
4. UI 侧 `on_connected`：无 resume 走 `apply_fresh_snapshot`（原 active session 仍存在则重新打开）；有 resume 走 `apply_resume_outcome`——`Replay` 由 reducer 按 sequence 续接重放事件、不闪全量重载；`SnapshotRequired` 先丢 stale 权威标记（审批卡、snapshot pendings、active runs、timeline 基线、blocked 集）再换基线，对仍存在的 active session 重分页并清其 unread（重分页后用户在看）、消失 session 的 unread 一并清除、仍存非 active session 的 unread 保留，active session 消失则置 `None`（UI 侧 `pending_scope_focus`，下一次 `render` 把焦点回退到 scope 触发器；`on_connected` 无 Window，不能当场 `window.focus`）；`UpToDate` 不碰 Timeline。三态文案（`Replay · a–b` 等）落 `status_hint` 与侧栏。断线（`Disconnected`）不清 `active_session_id` / unread / blocked——连接态与导航态解耦，Reconnect 后可续。
5. 打开会话（`open_session`）：`select_session` 无条件清 timeline / seen / tombstone / tool anchors 并恢复 snapshot 中该会话的 active run 与 pending approval → controller 按 `session_get{ timeline_after_sequence, timeline_limit: 500 }` 链式分页（至多 200 页）直到 `complete`，每页发 `TimelineLoaded`；分页期间先到的 live 事件由 reducer 按 sequence 去重。
6. 事件泵：`next_event_timeout(1s)` 循环——收到事件即记 last_acked（单调 max）、回 ack、投 `ControllerEvent::Event`；连续 15 个空闲 tick（≈15s）发一次 `heartbeat()` 保活（host `heartbeat_timeout` 30s，任意入站帧刷新；client io 为 AsyncMutex，支持泵内并发调用）。
7. 心跳失败或泵错误：清 client 槽、发 `Disconnected{reason}`；UI 置连接态并提示「Connection lost. Click Reconnect.」。用户点 Reconnect 重走 `start_connect`——带 last_acked 走 resume，不永远全新 Snapshot。

### 4.2 发送一次消息到流式渲染

1. Composer Enter（先判 `is_composing()`，IME 组合中的 Enter 属输入法确认直接返回）或 Send 点击 → `can_send`（Connected + 有 active session + 无进行中 run）+ 非空文本。
2. `run_start{ session_id, user_message[, provider, model] }`（模型取 `effective_model` = pending 优先于 selected，只影响下一轮）→ `Accepted{run_id}` → `MessageSent` 调 `note_session_run`（乐观写入 `active_runs`，active session 同时设 `active_run_id`）并清空输入框，不等 live `RunChanged`。
3. live 事件流（`RunChanged` / `AssistantDelta` / `ToolStarted` / `ToolOutput` / `ToolCompleted` / `Diagnostic`）经 `projection.apply_event`：时间线语义（sequence 去重、有序插入、assistant 按 message_id 增量合并、committed 替换 tombstone、tool 双键锚点回填）全在共享 reducer；本包只更新 UI 态（run 跟踪、审批卡、blocked 派生、非 active session 的 unread 标记、`model.switched` Diagnostic 确认模型切换）。
4. 时间线每次变化 `timeline_changed()` 递增代次；render 前 `sync_list` 对 `ListState` 统一 `reset(len + pending_approval)`（projection 有条目替换语义，splice 不安全）。跟随态（Bottom 对齐 `logical_scroll_top == None`）自动钉底；脱钩读史恢复 reset 前偏移，视口不跳；回底 = `scroll_to` 末项底 + 布局自动重挂。
5. run 终态（completed / cancelled / failed / interrupted）清 `active_run_id`（Composer 恢复可用）、清该 run 的审批卡，并触发 Changes 刷新；run 进行中由 1s 时钟驱动时长徽标重绘。

### 4.3 审批卡交互

1. live `ToolApprovalRequired{ run_id, tool_call_id, reason }` 或 snapshot `pending_tool_approvals` 段 → `pending_approval`（tool_name 从 reason 首段提取；snapshot 形态含 relative_path / preview）。
2. 按钮点击或 Cmd+1 / Cmd+2 / Cmd+3 → `tool_approve{ run_id, tool_call_id, decision }`，decision ∈ `approve_once` / `approve_for_run` / `deny`。
3. 清卡路径：对应 `ToolCompleted`、run 终态、历史 `ApprovalResponded`。无任何默认放行：不操作则永远 pending，断线时按钮禁用。

### 4.4 菜单开合与键盘激活语义

- 六种浮层（`MenuKind`：Grouping / Scope / Model / Entry(event_id) / WorkspaceConfirm / Activity）共用单一 `Option<MenuKind>` 状态位：开新即关旧、至多一个打开。
- 行级键盘激活（R3 Wave B Slice 4，design §3.6；Slice 5 P2b 扩展到 rail 聚焦 Button）：聚焦的 ListRow / Button 上裸 Enter / Space 在行级 `on_key_down` 直接调用与鼠标 click 同一激活 handler（task 行 `on_session_clicked`、项目头 `on_toggle_project`、grouping/scope/add-task/项目定向「+」按钮开菜单 / 新建），不走合成 click 兜底——GPUI 对聚焦 stateful 元素的 keyboard click 挂在 keyup 合成路径，真窗口注入取证不可达（[u2-nav slice3 enter-gap](../../ui-review/r3-wave-b/u2-nav/slice3/enter-gap.json)，Slice 4 修复后 enter_gap=0）。物理键盘下该合成 click 仍会到达：`pending_row_key_activate`（行）/ `pending_button_key_activate`（按钮）衔接标记吞除——Slice 5 起按「无按下位置 + 有未消费标记即吞」判定，行键 / 按钮 id 不匹配同样吞除（防跨行 / 跨元素误触发），鼠标真实 click（有按下位置）永不吞（防布尔反转回归）。带修饰键不接管；菜单打开时行级 / 按钮级让位（不 stop_propagation），Enter 由根节点菜单接管。
- Grouping / Scope / Model 菜单的键盘语义（R3 Wave B，design §3.6；Slice 5 修订）：菜单打开即接管（不再要求其触发器聚焦——Tab 移焦 / 外点后键盘仍归菜单），根节点 `on_key_down` 承接 ↑/↓ 移动键盘高亮（`menu_highlight`，wrap；None 时回落当前选中项）、Enter 走与点击同一 select 路径（`activate_menu_item`）、Escape 关闭并把焦点送回触发器；rail ↑/↓、行级与按钮级 Enter/Space 在菜单打开时让位（不 stop_propagation）保证冒泡到根节点。`pending_keyboard_menu_select` 衔接标记吞掉 Enter 选择后触发器同键 keyup 的合成 click，防止「选择即重开」（与外点衔接标记同构）。菜单任何关闭路径复位高亮。
- 关闭路径：选择选项 / 再点触发器 / Escape（根节点 `on_key_down` 冒泡承接；面板经 `deferred` 绘制不可聚焦，组件层不可达）/ 外点（`MenuPanel::dismiss_on_outside` 的 `on_mouse_down_out`）。
- 外点关闭先于触发器 click 到达时，以 `(MenuKind, 按下位置)` 衔接标记判定「同一次物理点击」——位置精确相等才视为关闭收尾不重开；键盘触发无位置永不误判。
- 面板 `occlude()` 拦截下层点击与滚轮（无穿透）；超高在 240px 内自滚。
- 归一化：model 菜单在 `can_switch_model` 翻假期间由 render 关闭；Entry 菜单在 timeline reset 前先关（锚点条目可能被虚拟化卸载）；Inspector 程序化展开前关闭悬浮菜单（防 ActivityPopover 叠面板）。

### 4.5 Fork 与分支切换

1. 条目「···」→ Fork。渲染层 gate：Connected + active session + `entry.is_fork_boundary()`；`on_fork` 入口再复核同三条件（双重防线）。
2. `session_fork{ session_id, parent_event_id }` → 响应 Data 提示 `session_id|branch_id`，否则重取 snapshot 挑 `updated_at_ms` 最新 → `SessionForked` → `open_session` 切入分支。
3. 同一 session 切 branch 也必须走 `select_session` 全量 reset（active branch 只存在 host 侧、不进 wire，UI 无从增量区分）。

### 4.6 Changes / Resources 拉取（epoch 防过期）

- 时机：页签切入、Inspector 展开、会话切换、run 终态、手动 ↻、ActivityPopover 摘要点击。
- 每次拉取递增 epoch 并随查询带出，响应原样带回；过期代次直接丢弃（`apply_files` / `apply_diff` / `apply_servers` 校验）。diff 响应还须匹配当前选中路径。
- 清单刷新后：选中文件仍在则重拉其 diff 保持两视图一致；选中文件消失则清空选中与 diff。
- 失败回写：仅当面板仍处 `Fetching` 才落 `Failed`（防旧请求失败覆盖新一轮），同时 `status_hint` 提示；未连接 / 无 workspace 诚实标 `not connected` / `no workspace`，不画演示数据。

### 4.7 协议消费面（wire method 与响应事件对照）

| 用户动作 | wire method | 结果（ControllerEvent） |
| --- | --- | --- |
| 连接 / 重连 | 握手 + resume + subscribe_all | `DesktopConnect{snapshot, resume, events}` |
| 打开会话 | `session_get`（分页查询） | `TimelineLoaded`（逐页） |
| 新建任务 | `session_create` → 重取 snapshot | `Snapshot` + `SessionCreated` |
| 发送消息 | `run_start` | `MessageSent{run_id}` |
| 取消 run | `run_cancel` | 经事件流 `RunChanged` 收敛 |
| 审批决策 | `tool_approve` | 经事件流收敛 |
| Fork | `session_fork` → 重取 snapshot | `Snapshot` + `SessionForked` |
| 终端 | `terminal_create` / `terminal_write` / `terminal_resize` | `TerminalCreated` / 流式 `TerminalOutput` |
| 模型目录 | `model_list` | `ModelsLoaded` |
| Changes | `diff_list_files` / `diff_get` | `DiffFilesLoaded` / `DiffContentLoaded`（带 epoch） |
| Resources | `mcp_list` | `McpServersLoaded`（带 epoch） |
| 任意失败 | — | `OperationFailed{action, reason}` |

domain id 类型未从 client re-export，命令 / 查询经冻结的 serde 形状（`method` / `params` JSON）构造，避免引入第二个业务依赖；`CommandSource::Automation` + `ActorIdentity::System` 仅为信封占位，服务端 host_stamp 统一覆盖为 LocalGui + LocalUser。

## 5. 契约与不变量

- **视觉基准事实源**：[../../../design/README.md](../../../design/README.md)（三张 1440×1024 基准图 + §8 组件规范）与 [../../gui-design.md](../../gui-design.md)（Surface 与连接协议消费约定）。theme token 已于 R2 Wave A（2026-08-27）按设计事实源 §2.1 冻结色板落地源码（证据 [../../ui-review/r2-wave-a/](../../ui-review/r2-wave-a/)）；组件级视觉还原（F-03~F-09）随 R2 后续 wave 推进。hover / active 只改背景，active 复用 hover 色。
- **审批 fail-closed**：无默认允许；决策只能来自显式点击或快捷键；断线禁用；run / tool 终态与 `ApprovalResponded` 清卡防幽灵审批。
- **`gui.token` fail-closed**：token 缺失、不可读或为空即连接失败，禁止无认证静默连接；错误信息只含路径，token 内容不落日志。
- **Enter / IME 语义**：keybinding 仅 `TextInput` 聚焦时生效；Enter 冒泡到 AppView 后结合 `is_composing()`（`marked_range` 存在即组合中）与发送可用性裁决；Shift+Enter 恒为换行；终端输入框同规则。
- **禁动符号**（R8 冻结面，bin 内测试钉住内容）：`APP_VIEW_KEYBINDINGS`、`install_keybindings`、`MAIN_PATH_TAB_STOP_IDS`、`resolve_new_task_workspace`。
- **依赖边界**：生产 `pawork-*` == `{pawork-client}`（deny-list 测试）；`projection.rs` 零 gpui / tokio / OS import；协议类型只经 client re-export。
- **时间线单一 reducer**：条目去重 / 合并 / 锚点 / resume 基线语义全部委托 `pawork_client::projection`（protocol 共享 reducer，`TimelineEntry` / `TimelineEntryKind` 直接 re-export）；本包只保留 UI 态与渲染分组。timeline 任何变化统一 `reset(count)`，禁 splice。
- **重连三态可见**：Replay / SnapshotRequired / UpToDate 必须以文字在侧栏区分（不只靠颜色）；仅 SnapshotRequired 换基线重分页。
- **TaskRail 状态点诚实语义（R3 Wave A + Wave B）**：`SessionLiveStatus` 三态——NeedsInput（该 session 有待审批）> Running（`active_runs` 成员，含 live `RunChanged` 非终态登记）> Blocked（R3 Wave B live 派生：该 session 最近一条 `RunChanged` 为终态且 state ∈ failed / interrupted，completed / cancelled 不算；同 session 任何其它 `RunChanged` 清除；快照重建清空——wire 无终态来源，Replay 重放终态事件可重新派生）；其余会话一律空心灰圆，wire 无每会话终态字段故不画终态绿点；apply_event 在 active-session 闸门前跨会话维护成员关系，终态按 run_id 移除并清 pendings。unread 为独立通道（`session_unread()`）：非 active session 的 Session-stream 活动事件（RunChanged / AssistantDelta / ToolStarted / ToolOutput / ToolCompleted / MessageSent / Diagnostic；MessageSent 为本地 composer 回执只属 active session，不经 wire）标记，`select_session` 清除，首连 / 快照重建不产生（无 last-seen 基线）。
- **诚实显示**：tokens / quota / tok/s 无权威来源一律 `—`；ContextMeter 只用 catalog 的 `context_window_tokens`；Changes / Resources 未拉取显 unavailable 而非 0；`now_ms` 由 UI 注入，投影层不读系统时钟。
- **终端约束**：`terminal_create` 的 cwd 只接受 workspace 相对路径（拒绝绝对路径、Windows 盘符前缀、`..` 分量）；终端面为滚动文本，无本地 PTY。
- **心跳配比**：15 空闲 tick（≈15s）对 host 30s 超时的节拍不可静默改动；断线不取消 Run。
- **窗口与焦点**：默认 1440×1024、最小 1080×720（`WINDOW_MIN_SIZE`，R2 Wave B 设计响应式底线——再窄击穿 Workspace ≥560 合同）；macOS 透明 titlebar（无白带、无重复标题栏，R2 Wave A）；启动聚焦 Composer；点击输入框显式拉回焦点（`track_focus` 自动聚焦不够，否则第二轮键盘 / IME / 粘贴进不来）。
- **Accessibility 单一语义源（ADR-042）**：`AppView` 只从自身 canonical UI 状态与布局 metric 构建显式 `AxTree`；壳层三栏几何与 render 共享 `shell_layout::resolve`（窄窗 rail=240 / Inspector 折叠态 AX bounds 不偏离实际布局）；稳定 identifier 与本地化 label 分离，macOS bridge 只做 AppKit 映射。AX press / focus / set-value 必须回到既有 AppView handler 与 enable gate，未知请求 fail-closed；disabled 控件不得发布可执行 action。触发器语义与可见路径一致（Inspector 折叠态先弹 ActivityPopover，摘要行才展开）；IME composing 中 AX Send 与键盘 Enter 同样不生效。新增可见交互须同批补节点、bounds、状态和 action 映射；非 macOS 当前为 no-op，不宣称已有平台 AX 实现。

## 6. 依赖关系

| 依赖 | 版本 / 形态 | 用途 |
| --- | --- | --- |
| `pawork-client` | path 依赖 | 唯一业务入口；re-export 协议 / 传输 / 共享投影 reducer 类型 |
| `gpui` | `= 0.2.2`（ADR-035 精确锁定） | UI 框架；升级须过 ADR |
| `smol` | 2 | UI 侧 channel 与 1s Timer |
| `tokio` | workspace（rt-multi-thread / macros / sync / time） | GUI Connection Protocol 异步宿主 |
| `serde_json` | workspace | 命令 / 查询构造与 snapshot 段、响应解析 |
| `unicode-segmentation` | 1 | 输入框 grapheme 光标边界 |
| `cocoa` / `objc` / `raw-window-handle` | macOS target only：`=0.26.0` / `0.2` / `0.6` | ADR-042 原生 AppKit AX bridge 与从 GPUI window 取得 `NSView`；不增加 `pawork-*` 业务依赖 |

- **被依赖**：无。独立二进制，不进 `pawork` CLI 的依赖闭包。
- **运行时对端**：`pawork gui serve`（host 侧 gui-server）。数据目录规则镜像 app（`PAWORK_DATA_DIR` →（Windows）`%LOCALAPPDATA%/pawork` → `~/.pawork` → 临时目录/pawork），但按分层约束**不**依赖 `pawork-app` crate。

## 7. 测试与验证资产

94 个测试全部内嵌于 bin target（`#[cfg(test)]` 模块；无 crate `tests/` 目录），按文件分布：

| 文件 | 数量 | 覆盖面 |
| --- | --- | --- |
| `main.rs` | 1 | `WINDOW_MIN_SIZE` 钉 1080×720 设计响应式底线（R2 Wave B） |
| `controller.rs` | 11 | token 缺失 fail-closed；`run_start` / `session_fork` / `terminal_*` / `diff_*` / `mcp_list` wire 形状钉死；cwd workspace 相对校验；last_acked 单调推进；capabilities 含 TerminalStreaming；diff 清单 / hunk 行 / git 信息 / MCP 响应解析（含无会话空响应与路径消失） |
| `projection.rs` | 26 | snapshot 重建与事件重放；审批卡随 run 终态清理；pending model 被 `model.switched` Diagnostic 确认；沙箱回退 Diagnostic 上时间线；snapshot active_runs 恢复取消目标与时长；`SessionLiveStatus` 优先级（NeedsInput > Running > Blocked > 无语义）、跨会话 live `RunChanged` 成员登记/终态移除、`note_session_run` 乐观 Running、后台 `ToolApprovalRequired`/`ToolCompleted` 闸门前入账（R3 Wave A）；Blocked live 派生与清除（failed / interrupted 记、非终态与 completed / cancelled 清、快照重建清空、Replay 重放再派生）、unread 通道全转换（六类活动事件标记 / active 排除 / select 清除 / 快照不产生 / Replay 重放标记）、断线保留 active+unread+blocked、`apply_snapshot_required` 导航回归（仍存 active 保留并清 unread、消失置 None、消失 session unread 清除）（R3 Wave B）；ContextMeter / RunStatus 诚实文案（F-13 定稿语序 + 竖线分隔，idle / 缺起始时长 / 运行中三态）；Reconnect 相位（仅 Disconnected / ConnectFailed）；空态引导可见条件（无 session + 无条目；Disconnected 保留条目不显示）；TaskRail 日期→项目分组与 Unassigned；scope 选项与空态；分组切换不改 active session；session_tree 扁平 / 分支双形态；同 session 切支 reset 基线（seen / tombstone / 锚点全清后重放重建）；TerminalOutput 追加与跨会话隔离；live tool 输出回填 running 条目；历史审批事件留痕；UI fixture 期望快照（`fixtures/ui/expected/snapshot.json`，再生步骤见该目录 README）重建 7 会话四桶分组、项目分组、pending 审批卡与 provider 状态 |
| `platform.rs` | 4 | socket / token 默认路径与 instance 命名；socket→token 推导；deny-list 恰为 `{pawork-client}`；扫描器覆盖别名 / target 表（负例含 dev-dependencies 排除） |
| `ui/mod.rs` | 9 | 键位表含审批与取消、任务循环与 next-needs-attention（cmd-alt-up / down / n）；主路径 tab_stop 全集与 rail 前缀 `RAIL_TAB_STOP_IDS`；All projects 新建须确认 workspace；`rail_focus_stops` 按 design §3.6 顺序（Projects / Timeline 双模式、折叠只留头部、桶限定键去重）；`cycle_index` 循环与空列表；`next_attention_session` 优先级（NeedsInput > Blocked > Unread、active 之后起算、active 自身排除）；键盘合成 click 吞除（有标记即吞、鼠标永不吞）；空态引导含 Cmd+Opt 提示 |
| `ui/accessibility.rs` | 3 | identifier 唯一与父子关系校验；focus 单一性；bounds hit-test 与无效树拒绝 |
| `ui/accessibility/app.rs` | 4 | 动态 identifier 的转义稳定且无 escape-marker 碰撞；项目 identifier 的日期桶限定与 Projects 模式稳定；Timeline 摘要截尾保持 UTF-8 边界；会话行 AX description 携带状态词（Running / Needs input / Blocked 同源 + 「· Unread」unread 语义，R3 Wave A/B） |
| `ui/accessibility/macos.rs` | 6 | 顶左 bounds → AppKit parent space 坐标转换；value-change diff；结构骨架比较（属性变化不触发重建）；settable/action 双门拒绝越权 value / focus 写入；disabled action fail-closed（macOS） |
| `ui/barriers.rs` | 1 | timeline_stable 重写且 settle_seq 单调、字段形状齐全；approval_visible 写入（含 tool 名）与消失删除；未启用（None）零写入 |
| `ui/theme.rs` | 5 | R2 Wave A WCAG 组合对比度定向断言：text.secondary×surface.hover≈4.82、tertiary/placeholder×surface.raised≈4.52、白字×accent.hover≈4.55、白字×success_hover≈4.61（容差 ±0.05）；placeholder×hover <4.5 钉住禁用组合；semantic.success_fg 落地为不透明 #74c94c；R3 Wave A TaskRail 几何与新字阶（TITLE=22 / BODY=18 / BODY_SM=17 / RAIL_*）钉冻结值 |
| `ui/shell_layout.rs` | 4 | R2 Wave A layout invariant：1280 阈值 rail 288↔240 切换；1440×1024 三栏合同（288/440/StatusBar 24）；1080×720 Inspector 强制折叠 + Workspace ≥560；resize 折叠与加宽恢复（后三个为 #[gpui::test]） |
| `ui/changes.rs` | 6 | ActivityPopover 摘要格式与单复数 / unavailable；二级页签默认 Files；epoch 拒过期与选中消失清 diff；diff 响应拒代次 / 路径不匹配；session_mismatch 判定矩阵 |
| `ui/inspector.rs` | 1 | 顶层页签默认 Terminal（与波 C 前单页行为连续） |
| `ui/resources.rs` | 1 | 默认 Idle 与 epoch 拒过期 |
| `ui/text_input.rs` | 2 | 多行粘贴的视觉行计数；AX set-value 替换内容并清除 IME marked range |
| `ui/u1_probe.rs` | 10 | R1 Wave C U1 spike：TestAppContext 实测矩阵（action 分发 / focus 断言 / keystrokes+input / mouse click / ScrollWheelEvent / resize / clipboard / 确定性 executor / debug_bounds 几何）；IME composing 与 AX 如实记为不支持，见 [../../ui-review/wave-c/u1/notes.md](../../ui-review/wave-c/u1/notes.md) |

**验证命令**：

```bash
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders
```

- `--bins`：本包是 bin-only（无 lib target），任务指南默认的 `--lib --tests` 匹配不到任何 target。
- `--features gpui/runtime_shaders`：gpui 默认构建在编译期调用 Metal 着色器编译器；开发机仅有 Xcode CLT 时缺 Metal Toolchain 会构建失败，runtime_shaders 把着色器编译推迟到运行时使本机可闭环（R8 起的标准口径，历波收口 28/28 → 43/43 → 53/53 → 62/62 → 74/74 → 78/78 → 84/84 → 88/88 → 93/93 → 94/94 绿）。

本包 dev-dependencies 为 `tempfile`（workspace `3`，仅服务 `ui/barriers.rs` 的临时目录测试）与 `gpui` dev 条目（`=0.2.2` + `test-support` feature，R1 Wave C 起；仅测试构建启用 TestAppContext/VisualTestContext，resolver v2 下不进生产二进制闭包），均不计入生产 deny-list。

**运行时验证资产**：`--probe`（连接 + snapshot + 模型目录一行摘要）与 `--probe-smoke`（流式回合 / 切模型 / 审批 / 取消 / 两次断线重连持久化 / `disconnect_survive`），配合隔离实例（`--instance` + `PAWORK_DATA_DIR`）在真实 host 上冒烟。R1 另提供 [scripts/ui-ax-dump.swift](../../../scripts/ui-ax-dump.swift) 做真窗口 AX tree / action 取证；补救前后证据分别见 [ax-gate](../../ui-review/wave-c/ax-gate/) 与 [ax-bridge](../../ui-review/wave-c/ax-bridge/)。端到端流程见 [../flows.md](../flows.md)；验证总策略见 [../README.md](../README.md)。
R3 Wave B 起 [scripts/ui-r3-wave-b-nav.sh](../../../scripts/ui-r3-wave-b-nav.sh) 以真窗口键盘注入驱动 22 相位导航回归（Tab 链 / 行级激活 / 菜单键盘 / 任务循环 / 断线重连 / Blocked·Unread live），证据与相位清单见 [u2-nav](../../ui-review/r3-wave-b/u2-nav/)；注入经 [scripts/ui-key-event.swift](../../../scripts/ui-key-event.swift)（CGEvent HID tap，合成键 flags 无条件赋值防 HID 状态粘滞继承）。

## 8. 注意事项与已知限制

- **gpui 前台执行器无 tokio reactor（历史崩溃教训）**：在 `cx.spawn` 的前台执行器上 await client 调用，会在 `receive_frame` 内部的 `tokio::time` 直接 panic（旧 R8 波 A 实证 exit 134，真窗口自始无法启动）。连接期握手 / ack / `subscribe_all` 与事件泵**必须**全部跑在 `runtime.spawn` 上，gpui 侧只经 channel 消费结果。`--probe-smoke` 走 `platform.block_on` 自带 runtime，暴露不了这类回归；R1 已由 [Wave D](../../ui-review/wave-d/notes.md) 建立真窗口启动门禁，R8 继续扩面。
- **Changes 面只读**（用户拍板 2026-08-24）：git_stage / HunkStageService 接线顺延 ADR 候选；`@` 补全浮层与「已加载规则」分区无 Host 出口（`@` 端到端展开在 host 侧 crates/app，不在本 crate）。
- **host `diff_*` 固定解析 latest 会话**：数据会话与当前查看会话不一致时，UI 以 banner「Showing changes for latest session X — not the active session.」与 popover 提示行如实标注，不静默张冠李戴。
- **渲染面自动门禁尚未完整**：R1 Wave C 已建立 U1 进程内探针与 macOS 真窗口 AX tree / semantic action / screenshot 来源；[Wave D](../../ui-review/wave-d/notes.md) 已补 State A 双基线、完整 visual diff、故意漂移与恢复。R2 Wave A（[证据](../../ui-review/r2-wave-a/notes.md)）落地透明 titlebar、v3 色板与 1440/1080 layout invariant，State A global 辅助 SSIM 0.336 → 0.650；当前视觉仍 0/9 zone 达到 0.99（内容组件 F-03~F-09 属后续 wave），菜单开合 / FollowScroll / hover / 虚拟化滚动 / DiffView 横滚仍主要依赖历史人工取证，须在 R2–R8 逐组件补齐。Entry 菜单在锚点条目被虚拟化卸载后状态与视觉短暂失联，需在 R7/R8 给出可恢复行为，不能沿用旧偏差接受。
- **ActivityPopover 触发器位置是已知偏差**：现实现由底部 StatusBar 右侧触发、向上展开；定稿为 Workspace Header 右上触发、向下展开约 320px 且不覆盖 Composer（[../../../design/README.md](../../../design/README.md) §5.1/§8.5、UI_Review F-12 / D-01）。迁移完成前 F-12 保持未通过；迁移落地后同批更新本文 §3.2 的 StatusBar 描述。
- **环境性断连**：显示器休眠 / App Nap 下心跳超时断连（Reconnect 横幅恢复）为宿主环境行为，非缺陷。
- **单主题**：仅深色 `dark()`；`Theme: Global` 是未来运行时主题挂载点，当前未 `set_global`。
- **文件尺寸口径**：`ui/mod.rs` 约 2060 行；这是工程结构口径，不构成新 UI 视觉或交互放行条件。
- **`text_input.rs` 血统**：改自 gpui 0.2.2 `examples/input.rs`（Apache-2.0），有意裁剪 ShowCharacterPalette / Copy / Cut / SelectAll / 拖选；后续补齐须对照上游而非自造。
- **FollowScroll 的滚轮时序假设**：`on_scroll_wheel` 直读已应用（未钳制）的 offset——依赖 vendored gpui 0.2.2 的 Bubble 相监听逆序分发（内部偏移应用先于用户监听）；升级 gpui 时须重核，做 delta 投影会把增量计两次。
- **UI fixture barrier 钩子（R1 Wave B，测试专用）**：启动读 env `PAWORK_UI_BARRIER_DIR`（main.rs；空值视同未设置，`--probe` / `--probe-smoke` 不发射）。未设置时全程零开销：不 spawn tick、无任何文件 IO。设置后由 ui/mod.rs 既有 1s tick 兼任发射点：已连接 && 无进行中 timeline 分页（`open_session` 置位、complete / `open session` 失败 / `Disconnected` 复位）&& 本 tick 窗口无 ControllerEvent 时重写 `<dir>/timeline_stable`（JSON 含 settle_seq 单调自增 / session_id / entry_count / at_ms / detail）；开始连接、打开会话或收到任一 ControllerEvent 时先删除旧 `timeline_stable` 与 `approval_visible`，防只等存在性的 driver 误收陈旧信号。`pending_approval` 存在且已稳定 → 重写 `approval_visible`（含 tool 名），消失 → 保持删除；目录不存在时由 `BarrierSink::new` 惰性创建。写入 tmp+rename 原子替换、任何 IO 失败静默跳过；`projection.rs` 保持纯状态机零 IO。controller 在未连接、无 TimelinePage 响应或翻页达到上限时发 `OperationFailed`，UI 据此复位分页状态。
