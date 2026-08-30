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

29 个 `.rs` 文件、约 18.9k 行，全部在 `[[bin]] pawork-desktop` target 内（无 lib target、无 crate `tests/` 目录）。

| 路径 | 行数 | 承载内容 |
| --- | --- | --- |
| `src/main.rs` | ~670 | 入口与手动 argv 解析（非 clap）；`PAWORK_UI_BARRIER_DIR` env 读取（空值视同未设置，None 全程零开销）；`run_app`（1440×1024 居中窗口 + `WINDOW_MIN_SIZE` 1080×720 最小尺寸——R2 Wave B 设计响应式底线，再窄击穿 Workspace ≥560 合同——+ 沉浸式 titlebar：TitlebarOptions appears_transparent，traffic lights 悬浮深色壳、内容视口贯通全窗，R2 Wave A；`install_keybindings`、聚焦 Composer、安装 macOS AX bridge）；`run_probe` / `run_probe_smoke` 无窗冒烟模式及其 `wait_for_*` 事件等待器；1 个测试 |
| `src/controller.rs` | ~1736 | `DesktopController`（connect / 事件泵 / 空闲心跳 / 全部 Command·Query 构造与响应解析）；`ControllerEvent` 对 terminal create 失败携带 workspace、对 diff 内容携带 Host 实际 session id，Changes/Resources 失败走可靠通道；`DiffFileSummary` / `DiffFileDetail` / `DiffHunkDetail` / `DiffLineDetail` / `GitDiffInfo` / `McpServerEntry` 视图模型；12 个测试 |
| `src/platform.rs` | ~230 | `Platform`（tokio multi_thread Runtime，`handle()` / `block_on()`）；`default_socket_path` / `socket_path_for_instance` / `token_path_for_instance` / `token_path_for_socket` 路径发现；deny-list 断言；4 个测试 |
| `src/projection.rs` | ~3700 | `DesktopProjection` 渲染适配投影；`ConnectionState` / `ResumeState` / `ResumeApply` / `TerminalState` / `PendingApproval` / `ModelEntry` / `ActiveRun` / `SessionSummary` / `WorkspaceSummary` / `SessionLiveStatus`；R6 Wave B 按 workspace 保存多个 terminal 与 create 失败，占位/回执确定性归并，首段 output 在 create 回执给出权威 workspace 前只缓存不上屏，UpToDate 合并非事件权威 snapshot 且历史输出不复活 exited/killed 终态；Timeline/TaskRail/Run 状态与 F-13 谓词；39 个测试 |
| `src/ui/mod.rs` | ~3108 | `AppView` 宿主：Workspace Header、Timeline、Composer 与 Inspector 三栏装配；R6 Wave A 的 Header Activity 触发器/Popover；R6 Wave B 将 terminal draft、pending write/create 按 workspace/terminal 归属；R7 Wave B 统一任务切换关旧菜单并把用户发起的 click/AX/快捷键路径聚焦 Composer、审批后焦点交接、Review changes 的 Inspector 选中页签焦点，并覆盖当前 task 激活与单可见 task cycling 的焦点边角；连接恢复后刷新已打开的 Changes/Resources，并消费携 workspace 的 terminal create 失败与携 session 的 diff 回执；键位、焦点链、菜单互斥、1s tick 与 barrier settle；15 个测试 |
| `src/ui/shell_layout.rs` | ~266 | R2 Wave A 壳层几何合同：`resolve`（唯一计算入口，render 与 AX 树共享）——宽窗 rail=288 / Inspector 440，窗口宽 ≤1279 时 rail=240 且 Inspector 强制折叠为抽屉（Workspace ≥560 底线）；R6 Wave A 为 Workspace/main/right-column 补 `min_w_0`，固定侧栏 `flex_none`，防长文本 min-content 把真实 Inspector 挤窄而 AX 仍报 440；rail 顶部 36px traffic-light 安全区占位；4 个 GPUI 布局测试含长行回归 |
| `src/ui/accessibility.rs` | ~410 | 平台无关 `AxTree` / `AxNode` / role / action / request / rect 模型，identifier 唯一性、层级、focus / hit-test 约束；非 macOS no-op facade；3 个测试 |
| `src/ui/accessibility/app.rs` | ~2071 | R4 Wave A 增 `header_ax`（标题 / branch / live 状态）与 `timeline_row_ax` 五类节点；R6 Wave A 将折叠态 `inspector-toggle` 与 ActivityPopover 语义树迁至 Header，右缘/向下锚点、320 高度和 render 共享 metrics，Inspector 顶层 100×58 与 Changes 二级 96×56 tab frame 同步；从 `AppView` canonical UI 状态与布局 metric 构建三栏语义树；壳层几何与 render 共享 `shell_layout::resolve`；TaskRail 几何与 render 共享 `metrics::RAIL_*`，连接/会话状态文案同源；空态引导、run-status、TaskRail grouping/collapse、稳定动态 identifier、Composer/Send/Cancel、press/focus/set-value 白名单均与 render/业务 gate 同源；6 个测试 |
| `src/ui/accessibility/macos.rs` | ~940 | ADR-042 AppKit bridge：`GPUIView` AX root、`NSAccessibilityElement` 虚拟元素、frame / parent / hit-test / focus / notification / retain-release、settable/action 双门与 action 回调；结构不变（identifier/role/press 能力/子树形状）时原位刷新既有 element 而非整树重建，内部树同步 super 直调不触发 action 回调；6 个 macOS 测试 |
| `src/ui/barriers.rs` | ~175 | UI fixture barrier 发射器（R1 Wave B）：`BarrierSink` 读 `PAWORK_UI_BARRIER_DIR`（None 零开销直通）；`timeline_stable`（settle_seq 单调自增 / session_id / entry_count）重写与 `approval_visible` 写/删；tmp+rename 原子替换、IO 失败静默；1 个测试 |
| `src/ui/theme.rs` | ~552 | 深色单主题 token：六组 29 色 + 字阶 `font`（XS=11 / SM=12 / BASE=13 / MONO="Menlo"，R3 Wave A 增 TITLE=22 / BODY=18 / BODY_SM=17）+ `metrics` 尺寸常量（含 TaskRail、Header/Timeline、Composer；R6 Wave A 增 Inspector 顶层 100×58、Changes 二级 96×56、2px 选中下划线、ActivityPopover 320×320 合同；R7 Wave C 增连接槽与全局「+」的 8px 间隔）；静态 `dark()` 访问器；`impl Global` 仅为未来主题挂载点；8 个定向测试 |
| `src/ui/timeline.rs` | ~429 | Timeline 容器：gpui `list()` 变高虚拟化（R4 Wave A 自 Bottom 钉底改为 Top 对齐 + 显式跟随，F-06 短会话从 Header 下开始：贴底判定走滚动事件事实——`visible_range` 覆盖末项即贴底，handler 内不触 ListState 防 gpui 写借用重入 panic；`sync_list` 跟随态 scroll_to 末项、脱钩恢复钳制偏移）；`timeline_row_element` 五类渲染行组装（`timeline_rows()` 同源）、行距量图冻结值、可读列 max_w 618 左对齐；空态引导（无 active session 且条目数为 0 时居中 tertiary 一句，R2 Wave B）；`install_scroll_follow`（脱钩检测）；`sync_list`（统一 reset、脱钩恢复钳制偏移、Entry 菜单 close-on-reset）；`TIMELINE_OVERDRAW`=200px；回底控件接线 |
| `src/ui/timeline_entry.rs` | ~843 | R4 Wave A 重写：F-07 消息层级（You/Pawork 标签行 + 时间、18px 正文行高 24、段落/• 列表两级切分）、F-08 `tool_group_element`（r5 描边面板、52 行高 / 2px 分隔线、状态 ✓/词诚实映射 succeeded→Completed 其余原文、无耗时字段不画）、`run_summary_element`（Ø40 状态圆随终态：Completed 绿 ✓ / Failed danger ✕ / Cancelled —；Ready for review + Review changes 主按钮，Open in editor 无 capability 不画）、`run_footer_element`、`error_entry_element`；`display_time`（epoch millis 串 → `relative_activity` 相对词 now/Nm/Nh/Nd，非法串原样兜底，render 与 AX 同源）；条目「···」fork 菜单迁入 message/error（identifier 冻结）；R7 Wave B 的 Review changes 展开后聚焦 Changes 选中页签，`on_fork` 入口级复核并在接受后聚焦 Composer；8 个测试 |
| `src/ui/approval_card.rs` | ~122 | 审批卡：警示卡 + Allow once / Allow for run / Deny 三按钮；app 级 focus handle（虚拟化卸载不丢失）；禁用原因 tooltip；R7 Wave B 的 mouse / keyboard / AX 三路径汇入同一 gate，决策后关闭旧菜单并把焦点交回 Composer |
| `src/ui/input_area.rs` | ~545 | Composer（R5 Wave A 两行）：行 1 TextInput 单行常态 28px、多行向上增长并按面板 220 预算 clamp；行 2 footer（model Dropdown `provider / display_name` 触发器 max_w 220 truncate、只读 workspace Label max_w truncate、ContextMeter 文本、瞬态 `status_hint` Label、flex spacer、32×32 Send/Cancel 同槽 element id `composer-action`）；提示行删除，placeholder 只走状态机，Forked / 发送失败等瞬态反馈落 footer Label；WorkspaceConfirm 浮层保留；reasoning / 附件 / queue 诚实不画 |
| `src/ui/inspector.rs` | ~281 | Inspector 面板：顶层 Changes / Terminal / Resources 三页签（`InspectorTab`，R6 Wave A 起默认 Changes）；顶层固定 100×58、18px 字与 2px 选中下划线，hover / active 只取 `surface.raised` 背景且不改几何；Terminal 页（cwd / size 行、FollowScroll 输出区 + 回底、终端输入 + Start/Size）；无输出占位文案 `TERMINAL_EMPTY_OUTPUT`（视觉与 AX 共源，R2 Wave B）；`ensure_terminal` 懒创建；1 个测试 |
| `src/ui/changes.rs` | ~871 | Changes 面：Files / Summary 二级页签固定 96×56、17px 字与 2px 选中下划线；`ChangesPanelState`（双 epoch 防过期）、文件清单、DiffView（hunk 着色 + 按最长行建立真实横滚 extent）、latest-session mismatch fail-closed 与诚实 scope；折叠态 Header ActivityPopover 标题/Changes 权威摘要，不伪造 Agent；7 个测试 |
| `src/ui/resources.rs` | ~210 | Resources 页：MCP server 只读表 + `ResourcesPanelState`（epoch 防过期）+ 手动刷新；1 个测试 |
| `src/ui/task_rail.rs` | ~775 | Sessions 侧栏（F-03/F-04 定稿，R3 Wave A；R3 Wave B 增键盘导航与状态扩展）：顶部三行——标题行「Pawork」22px + ghost grouping 角标 28×28、全宽 raised scope 行（h36 / 1px 描边 / r4 / 18px 左对齐）、连接行（Ø10 状态点 + `connection_status_label` 17px + 28×28 全局「+」；R7 Wave C 将文案槽定为 rail 内容宽 − 全局「+」− 共享 8px 间隔，并在槽内 `overflow_hidden + truncate`，窄窗长断线原因显示省略号且不越过合同间隙，AX 仍提供完整状态值）；grouping / scope 菜单（MenuRow 键盘高亮）、日期桶头（18 medium secondary）与项目块（chevron + 名称 + 右对齐计数 + 28×28 定向「+」）折叠、44px 任务行（状态点 + 标题 `.truncate()` + 17px 相对时间右对齐，选中 raised + r4；Blocked=danger 红实心、unread 标题 SEMIBOLD 同字号不改几何）、Reconnect 按钮（仅 Disconnected / ConnectFailed，Connecting 不显示，R2 Wave B）、连接状态徽标、「Local」页脚（honest-hidden，TR-12）；桶头 / 项目头 / 任务行平铺为滚动容器直接子元素（`ScrollHandle::scroll_to_item` 拿行级 bounds，grouping/scope 切换后把 active task 滚到可见，折叠项目退回头部行）；rail 内键盘导航 `handle_rail_navigation_key`（↑/↓ 沿焦点链移焦 clamp 不 wrap，项目头 ←/→ 收起/展开已处目标态 no-op，Enter/Space 经 ListRow 行级 key_down 直接调激活 handler（Slice 4）；Slice 5 菜单打开时 rail 让位不接管；rail 聚焦 Button——grouping/scope/add-task/项目定向「+」——裸 Enter/Space 行级激活同 click 路径，菜单已开时让位给根节点菜单 Enter 接管），带修饰键不接管 |
| `src/ui/text_input.rs` | ~1300 | `TextInput`（Composer / 终端共用）：内容 / 动态 placeholder / IME marked_range / UTF-16 映射 / 视口 max_h + overflow scroll（TextElement 按完整内容高布局，视口由父容器 max_h 兑现，caret 滚进视口按 ScrollHandle 容器高计算；鼠标与 IME 坐标映射基于归一化布局原点 content_bounds——prepaint 时 origin 减 element_offset，与帧时序无关——再减 scroll offset，行高取 paint 时 last_line_height）/ 选择复制剪切 / Undo Redo / reset_text 草稿恢复；10 个测试 |
| `src/ui/u1_probe.rs` | ~410 | R1 Wave C U1 spike：真实 TextInput/Button/overflow 探针；R5 Wave B 增 SelectAll/Copy/Cut/Undo/Redo、IME commit 单次入栈（真实 EntityInputHandler 路径）与空输入不可发送、Wave B 键位（含 Shift-Enter）经 keystroke→keymap→action 真实链路覆盖；14 个测试 |
| `src/ui/components/mod.rs` | ~10 | 组件族模块声明 |
| `src/ui/components/button.rs` | ~340 | `Button`：六 variant（Primary / Ghost / Danger / Success / Raised / IconCircle；Icon 于 R3 Wave A 退役，无消费点；IconCircle 为 Composer 32×32 圆形动作槽）的底色·文字色·hover/active 映射；`ButtonPadding` 四档；`bordered()` / `radius()` / `center()` / `vcenter()` 描边与对齐修饰（R3 Wave A）；tab_stop + track_focus + 聚焦描边三件套；tooltip；`on_activate` 行级键盘激活（Slice 5 P2b：聚焦按钮裸 Enter/Space 直接调激活 handler 禁合成 click；disabled 不激活；是否 stop 由调用方决定） |
| `src/ui/components/dropdown.rs` | ~231 | `Dropdown`（触发器 + `deferred(anchored())` 浮层；可选局部 Corner/Point 锚点，Activity 用 TopRight 让右缘对齐）、`MenuPanel`（`occlude()` + `on_mouse_down_out`；默认 `MENU_MAX_HEIGHT`=240px，可按冻结浮层合同覆盖）、`MenuRow`（选中 / 禁用 / hover / 键盘高亮语义）、`ANCHOR_GAP_Y`=4px |
| `src/ui/components/follow_scroll.rs` | ~90 | `FollowScroll`（`ScrollHandle` + 跟随位：贴底判定 / 脱钩 / 重挂；现仅终端使用）与 `BackToBottom` 回底控件容器（绝对定位右下） |
| `src/ui/components/label.rs` | ~70 | `Label`（单行文本，token 化字号 / 颜色）与 `Badge`（状态徽标别名，默认 XS + text.secondary） |
| `src/ui/components/list_row.rs` | ~130 | `ListRow`：Task 行（选中态底色，水平 `px_2`）与 ProjectHeader 行两形态，行高 44 + 垂直居中（R3 Wave A）；`min_w_0` 保证子项 truncate 拿到确定宽度；R3 Wave B 增 `track_focus` 键盘焦点三件套（tab_stop + track_focus + 聚焦 accent 描边，同 Button 模式）与 `on_activate` 行级键盘激活（裸 Enter / Space key_down 直接调与 click 同一激活 handler，Slice 4；禁合成 click 兜底；Slice 5 起 stop_propagation 由调用方 handler 决定——菜单打开时让位给根节点菜单 Enter 接管） |
| `src/ui/components/panel.rs` | ~78 | `Panel`：`side_right`（TaskRail，右描边 + gap/p-2）与 `side_left`（Inspector，左描边）固定宽面板壳；固定宽时 `flex_none`，禁止 Workspace 长内容把侧栏挤窄 |
| `src/ui/components/status_bar.rs` | ~70 | `StatusBar`：底部 24px 状态行容器（顶描边 + XS 次要文字）；F-13 信息串 `centered()` 行内绝对居中；R6 Wave A 起不再承载 Inspector/Activity 动作 |

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
  - 任务行（44px，选中 raised + 圆角 4）：状态点 Ø10（Needs input=琥珀实心 = 有待审批；Running=accent 蓝实心 = 该 session 有 active run；Blocked=danger 红实心 = R3 Wave B live 派生的 failed / interrupted 终态，优先级 Needs input > Running > Blocked；其余空心灰圆不声明语义；wire 无每会话终态字段故不画终态绿点）、标题单行 `.truncate()`（unread 时 SEMIBOLD，同字号同行高不改几何，不加 dot / 徽标）、相对时间 17px 右对齐（now / Nm / Nh / Nd）；任务 click、行级 Enter、AX press、Cmd+Opt+↑/↓ 与 Cmd+Opt+N 都先关闭旧菜单并在切换后聚焦 Composer。激活当前 task 不重开 session 但仍关闭菜单；仅一个可见 task 时 cycling 不重开 session，仍聚焦 Composer。
  - grouping / scope 切换不改 active session 也不动分组展开状态，下一次 render 把 active task 滚动到可见（项目折叠时退回头部行；active 被 scope 过滤则诚实跳过）。
  - 底部账户区（F-04，TR-12 honest-hidden）：只保留「Local」本机身份行；头像 / 姓名 / quota / 组织等无权威来源元素一律不画。
- **Timeline（中栏上）**：虚拟化列表渲染五类条目——`You:` 用户消息、`Assistant:` 助手消息（流式增量合并为一条）、工具调用（`name · status` + 可选输出摘要）、运行状态行、`Error:` 错误行。空态（无 active session 且条目数为 0）居中一句 tertiary 引导「Select a task from the rail, or press Cmd+N to start a new one. Cycle tasks with Cmd+Opt+↓ / Cmd+Opt+↑, or jump to the next task that needs attention with Cmd+Opt+N.」（AX 同步 `workspace-empty-hint` 只读节点；Disconnected 保留旧条目时不显示）。每条右侧「···」菜单含 Fork（仅 reducer 判定的闭合 run 边界可用；不可用时灰字禁用行；接受后聚焦 Composer）。用户上滚脱钩后右下浮出「↓ 回到底部」。
- **审批卡**：`pending_approval` 存在时作为 timeline 末项渲染——警示底色卡片（`Approval · {tool}` / reason / 可选 preview detail）+ 三按钮 Allow once（Cmd+1 / Cmd+Return，Primary）、Allow for run（Cmd+2，Success）、Deny（Cmd+3，Danger）；断线时禁用且 tooltip 给出原因。显式决策由 mouse / keyboard / AX 统一复核 gate，发出后关闭旧菜单并把焦点交回 Composer，避免卡片卸载后悬挂焦点。
- **Composer（中栏下，R5 Wave A / F-09）**
  - 面板常态总高 88–94px（`COMPOSER_PANEL_MIN_HEIGHT=88`，不是输入框 min）；增长上限 220px。两行：行 1 TextInput 单行约 28px（含 inset），多行向上增长；行 2 footer `items_center`，控件高 28–32px。
  - footer：model Dropdown 触发器（`provider / display_name`，无 reasoning 后缀；max_w≈220 truncate；run 进行中 / 目录未加载 / 断线禁用并 tooltip 给原因）→ workspace 只读 Label（不可点、无 chevron，max_w truncate）→ ContextMeter 文本（`Context · — / {window}` 或 `unavailable`，不画进度条）→ 瞬态 `status_hint` Label（Forked / Starting terminal / 发送失败等，仅 `status_hint.is_some()` 时渲染，max_w≈360 truncate）→ flex_1 spacer → 32×32 动作槽。
  - 动作槽单按钮：视觉 element id 统一 `composer-action`，单一 `composer_action_focus`。idle/disconnected/no-session 显示 Send（32×32 圆形 Primary，↑；可用 tooltip「Send message (Enter)」；空/纯空白、无 session、断线、running 均 disabled + tooltip 给原因）；running 显示 Cancel（同槽 32×32 Danger，✕，tooltip「Cancel run (Cmd+.)」）。Send 点击与 AX press 均先判 `is_composing()`，组合中不发送。AX 节点 id 仍为 send/cancel 随态互换。状态切换两按钮同槽互换，面板几何与锚点零位移。
  - per-session 草稿：`HashMap<session_id, String>` + 无 session 独立槽；`open_session` 切换前 stash 当前 Composer 文本、切换后 `reset_text` 恢复（无则空，清 undo）；`MessageSent` 成功清该 session 草稿（可见 Composer 仅在回执属于 active session 时清空）；断线不动草稿；终端 TextInput 不参与。发送清空走 `clear()` 入 undo 栈，发送后 Undo 可恢复上一条文本。超长文本由父容器 max_h + overflow_y_scroll 承载，caret 滚进视口，面板总高仍受 88–94 / 220 合同约束。
  - 提示行删除。空输入 placeholder 只走状态机（不被 `status_hint` 覆盖）：idle=`Message Pawork… (Enter to send, Shift+Enter for newline)`；running=`Run in progress — sending is disabled. Cancel remains available.`；无 session=`Open a session to send messages.`；connecting/disconnected/failed 沿用既有文案。瞬态反馈（Forked / 发送失败等）落 footer Label，发送失败在输入非空时也可见。非空输入时状态原因仍由 tooltip + AX 承载。
  - 诚实缺省：不画 reasoning、附件/纸夹、follow-up/queue；ContextMeter 维持文本；workspace 只读。
  - All projects 范围下新建任务先弹 WorkspaceConfirm 浮层选定 workspace（`resolve_new_task_workspace` 判定）。
- **Inspector（右栏，cmd-i 开合）**：顶层三页签，默认 Changes；顶层条 58px / 每项 100px / 18px 字，Changes 内二级条 56px / 每项 96px / 17px 字，选中态均为底部 2px accent 下划线；各页滚动状态独立保留。
  - Changes：Files / Summary 二级页签 + ↻ 手动刷新。Files = 文件清单（路径 · status · `+A/−D`，≤200px 内滚动）+ DiffView（等宽 Menlo；hunk 头 raised 底 secondary 字；addition 行 success_bg 底 / deletion 行 danger_bg 底 / context 行 panel 底；长行 `overflow_x_scroll` 横滚不折行；binary 显示「Binary file — not rendered.」）。Summary = 七字段行（Session / Files / Lines / By status / Branch / Dirty files / Work dir，缺失显 unknown）。数据会话 ≠ 查看会话时顶端 banner 如实标注。
  - Terminal：host 流式 `TerminalOutput` 滚动文本（非 VT100、无本地 PTY）+ cwd 与 `列×行` 尺寸行 + 终端输入（Enter 写入，未启动时先懒创建）+ Start / Size 按钮 + 脱钩回底控件。
  - Resources：MCP server 只读表（name + state 徽标，`failed` 红字；`transport · N tools[ · last_error]` 次行）+ ↻ 刷新。
- **StatusBar + ActivityPopover（R6 Wave A）**：底部 StatusBar 只居中 RunStatusBar 徽标 `Task — tokens | Quota unavailable | — tok/s | Run {mm:ss|—|idle}`（F-13 定稿语序与竖线分隔；缺权威来源一律 `—`，不伪造）。Inspector 展开态由面板内右上 `inspector-collapse` 折叠；折叠态 Workspace Header 最右 40×37 槽显示 `inspector-toggle` Activity，点击后在触发器下方以右缘对齐展开约 320×320 Popover：标题 Activity、Changes 标题与权威摘要 `N file(s) · +A/−D` 或 `unavailable`；点击摘要或 Run Summary 的 Review changes 均展开 Inspector、定位 Changes 并聚焦当前选中的 Changes 顶层页签；仅有 latest 会话差异时加来源说明。Agent/Add tool 无 capability 不画。

### 3.3 键盘与焦点

| 上下文 | 键 | 动作 |
| --- | --- | --- |
| TextInput | enter | SendMessage（冒泡到 AppView 裁决） |
| TextInput | shift-enter | NewLine |
| TextInput | backspace / delete / left / right / home / end | 对应编辑动作（home / end = 文档首尾，macOS 约定） |
| TextInput | cmd-v / ctrl-v | Paste（`\r\n` 归一为 `\n`） |
| TextInput | shift-left / shift-right / shift-home / shift-end | SelectLeft / SelectRight / SelectToLineStart / SelectToLineEnd |
| TextInput | cmd-a / ctrl-a | SelectAll |
| TextInput | cmd-c / ctrl-c | Copy |
| TextInput | cmd-x / ctrl-x | Cut |
| TextInput | cmd-z / ctrl-z | Undo |
| TextInput | cmd-shift-z / ctrl-shift-z | Redo |
| AppView | cmd-. | CancelRun |
| AppView | cmd-enter / cmd-1 | ApproveOnce |
| AppView | cmd-2 | ApproveForRun |
| AppView | cmd-3 | Deny |
| AppView | cmd-n | NewTask |
| AppView | cmd-i | ToggleInspector |
| AppView | cmd-alt-up / cmd-alt-down | TaskCycleUp / TaskCycleDown——按当前 rail 可见顺序循环切换 active task（空列表 no-op） |
| AppView | cmd-alt-n | NextNeedsAttention——按 rail 顺序找下一个 NeedsInput > Blocked > Unread 会话并打开；无候选 status_hint 提示「No task needs attention.」 |
| 浮层菜单（Grouping / Scope / Model / WorkspaceConfirm / Entry / Activity 打开时，不要求触发器聚焦） | up / down | 移动键盘高亮（wrap；未移动时从当前选中项起算，菜单关闭复位；单行菜单为 no-op） |
| 浮层菜单（同上） | enter | 选择高亮行（等价点击对应 MenuRow；触发器 keyup 合成 click 由衔接标记吞掉防重开） |
| TaskRail 列表 | up / down | 焦点沿 §3.6 停靠链步进（clamp 到两端，不 wrap；带修饰键不接管） |
| TaskRail 列表 | enter / space | 激活聚焦行：ListRow 行级 key_down 直接调用与 click 同一激活 handler（打开 task / 展开收起项目；R3 Wave B Slice 4 修复——GPUI keyup 合成 keyboard click 在真窗口不可达，不以合成 click 兜底；激活后的同键 keyup 合成 click 由 `pending_row_key_activate` 衔接标记吞掉防双触发——Slice 5 起标记不匹配同样吞除防跨行误触发；Grouping/Scope/Model 菜单打开时行级让位，Enter 由根节点菜单接管） |
| TaskRail 项目头聚焦时 | left / right | 收起 / 展开（已处目标态 no-op） |
| 根节点 | tab / shift-tab | focus_next / focus_prev——沿 tab_index 档位链走焦（GPUI 无默认 tab cycle；macOS 上 NSWindow 在 sendEvent 层吞掉裸 Tab，keyDown 路径收不到，主机制是 `install_appkit_tab_monitor` 的 NSEvent 本地监听器在派发前截获并调 `window.focus_next()` / `focus_prev()`，根节点 on_key_down 的 Tab 分支保留作非 macOS / 监听器失效后备；带 cmd/ctrl/alt 的组合键放行不接管） |
| 根节点 | escape | 浮层菜单（任意 `MenuKind`）打开时关闭并把焦点送回触发器（不要求触发器聚焦）；其余情况关闭当前浮层菜单 |

主路径按钮（`MAIN_PATH_TAB_STOP_IDS` 九项：approve-once / approve-for-run / approve-deny / composer-action / add-task / header-new-task / reconnect / model-picker / timeline-back-to-bottom）挂 tab_stop + track_focus + 聚焦描边三件套；Timeline 行级动作（Review changes `run-review-<event_id>`、Entry 菜单 `entry-menu-<event_id>`）按 event_id 懒建行级 FocusHandle，不进静态清单。Composer 动作槽视觉 element id 为 `composer-action` 单槽；AX identifier idle=`send` / running=`cancel` 随态互换。

rail 聚焦 Button（grouping / scope / add-task / reconnect / 项目定向「+」）上裸 Enter / Space 为行级键盘激活（Slice 5 P2b）：key_down 直接调用与 click 同一激活 handler（开菜单 / 新建 / 重连），禁合成 click 兜底；激活后的同键 keyup 合成 click 由 `pending_button_key_activate` 衔接标记吞掉（防「刚开的菜单被闪关」与「重复新建」）；浮层菜单已开时按钮让位，Enter 由根节点菜单接管；disabled 不激活。

Tab 焦点顺序（design §3.6，R3 Wave B）：rail 前缀三档 `RAIL_TAB_STOP_IDS`（-20/-19/-18：project-scope → task-rail-grouping → add-task）→ reconnect -17 档（R7 Wave A 接入；仅断线态渲染，不渲染时自动退出 Tab 链）→ rail 行级 -16 档（项目头 / 定向 ProjectAddTaskButton / task 行，按当前分组渲染序；折叠项目只保留头部）→ 主路径 `MAIN_PATH_TAB_STOP_IDS` 0 档 → composer `COMPOSER_TAB_INDEX` 1 档链尾（wrap 回 rail 首停）。Tab / Shift-Tab 经 AppKit 本地监听器（后备：根节点 on_key_down）映射 `window.focus_next()` / `focus_prev()` 真实可走（Slice 4）；菜单键盘导航在浮层菜单打开时即接管（Slice 5 起不再要求触发器聚焦；rail 与行级 / 按钮级激活在菜单打开时让位，冒泡到根节点裁决），Escape 关闭一律回焦触发器。

AX 焦点口径（R7 Wave A）：浮层菜单打开时，触发器 AX 节点让出 focused（发布谓词为 `open_menu.is_none() && trigger_focus.is_focused(window)`），高亮菜单项成为树内唯一 focused 节点（macOS 菜单惯例；原生 AX focused 随高亮移动）；WorkspaceConfirm 浮层行同样接入 `focused(ix == highlight)` 高亮体系。Timeline 行级动作（Run 摘要卡 Review changes、条目「···」菜单）行级 `on_activate` 与 click 同 handler 同 enable gate，键盘路径按 event_id 复核防虚拟化越权。

## 4. 核心行为与数据流

### 4.1 启动 → 连接 → snapshot → 分页 timeline → live 事件 → 断线 Reconnect

1. `AppView::new` 即 `start_connect`；`DesktopController::connect` 先按 socket 文件名推导 token 路径（`pawork-gui-X.sock` → `gui-X.token`）并读 `gui.token`——缺失 / 不可读 / 为空即整个连接失败（fail-closed）。
2. 建 512 容量 `smol::channel`；`LocalTransport` + `ConnectOptions{ timeout 10s, client_label "pawork-desktop", 帧上限 1MiB }`；带上内存中的 last_acked（若有）。
3. **在 `runtime.spawn` 内**执行 `GuiClient::connect_with_resume_config`（含握手与能力宣告）→ 取 `initial_snapshot` → 按 resume 三态 ack：首连记录并 ack `snapshot_sequence`；`Replay` 记录并 ack `through_sequence`；`UpToDate` 只记录 `current_sequence`；`SnapshotRequired` 换用 `outcome.snapshot`（无则回退握手首帧）再记录并 ack → `subscribe_all`。连接期任何 client 调用都不得落在 gpui 前台执行器上（见 §8 崩溃教训）。
4. UI 侧 `on_connected`：无 resume 走 `apply_fresh_snapshot`（原 active session 仍存在则重新打开）；有 resume 走 `apply_resume_outcome`——`Replay` 由 reducer 按 sequence 续接重放事件、不闪全量重载；`SnapshotRequired` 先丢 stale 权威标记再换基线并重分页；`UpToDate` 保留 Timeline，但合并握手 snapshot 中无法由事件恢复的权威状态（尤其 terminal 终态）。任何成功重连都刷新当前打开的 Changes/Resources；断线不清 `active_session_id` / unread / blocked。
5. 打开会话（`open_session`）：先关闭任何旧 `MenuKind`，再由 `select_session` 无条件清 timeline / seen / tombstone / tool anchors 并恢复 snapshot 中该会话的 active run 与 pending approval → controller 按 `session_get{ timeline_after_sequence, timeline_limit: 500 }` 链式分页（至多 200 页）直到 `complete`，每页发 `TimelineLoaded`；分页期间先到的 live 事件由 reducer 按 sequence 去重。用户发起的 task click / 行级 Enter / AX press / cycling / next-needs-attention 调用方在切换后统一聚焦 Composer。
6. 事件泵：`next_event_timeout(1s)` 循环——收到事件即记 last_acked（单调 max）、回 ack、投 `ControllerEvent::Event`；连续 15 个空闲 tick（≈15s）发一次 `heartbeat()` 保活（host `heartbeat_timeout` 30s，任意入站帧刷新；client io 为 AsyncMutex，支持泵内并发调用）。
7. 心跳失败或泵错误：清 client 槽、发 `Disconnected{reason}`；UI 置连接态并提示「Connection lost. Click Reconnect.」。用户点 Reconnect 重走 `start_connect`——带 last_acked 走 resume，不永远全新 Snapshot。

### 4.2 发送一次消息到流式渲染

1. Composer Enter（先判 `is_composing()`，IME 组合中的 Enter 属输入法确认直接返回）或 Send 点击 / AX Send press（同样先判 composing）→ `can_send`（Connected + 有 active session + 无进行中 run + 文本 trim 非空）才可点；空/纯空白 Send disabled，tooltip「Message is empty.」。
2. `run_start{ session_id, user_message[, provider, model] }`（模型取 `effective_model` = pending 优先于 selected，只影响下一轮）→ `Accepted{run_id}` → `MessageSent`（回执携带发送文本）调 `note_session_run`（乐观写入 `active_runs`，active session 同时设 `active_run_id`）与 `note_user_echo`（本地乐观回显：active session 立即上屏 UserMessage 行并 bump 时间线代次）并清空输入框，不等 live `RunChanged`；wire 无用户消息事件（`MessageCommitted` 不进实时流），重选 / 重连后由快照重放的持久化行替换回显行。
3. live 事件流（`RunChanged` / `AssistantDelta` / `ToolStarted` / `ToolOutput` / `ToolCompleted` / `Diagnostic`）经 `projection.apply_event`：时间线语义（sequence 去重、有序插入、assistant 按 message_id 增量合并、committed 替换 tombstone、tool 双键锚点回填）全在共享 reducer；本包只更新 UI 态（run 跟踪、审批卡、blocked 派生、非 active session 的 unread 标记、`model.switched` Diagnostic 确认模型切换）。
4. 时间线每次变化 `timeline_changed()` 递增代次；render 前 `sync_list` 对 `ListState` 统一 `reset(len + pending_approval)`（projection 有条目替换语义，splice 不安全）。R4 Wave A 起为 Top 对齐 + 显式跟随：跟随态由 `timeline_following` 单一表达（滚动事件 `visible_range` 覆盖末项即贴底），reset 后跟随臂显式 `scroll_to` 末项底；脱钩读史恢复 reset 前偏移（item_ix 越界钳制），视口不跳；回底 = BackToBottom / 滚回底部重挂。
5. run 终态（completed / cancelled / failed / interrupted）清 `active_run_id`（Composer 恢复可用）、清该 run 的审批卡，并触发 Changes 刷新；run 进行中由 1s 时钟驱动时长徽标重绘。

### 4.3 审批卡交互

1. live `ToolApprovalRequired{ run_id, tool_call_id, reason }` 或 snapshot `pending_tool_approvals` 段 → `pending_approval`（tool_name 从 reason 首段提取；snapshot 形态含 relative_path / preview）。
2. 按钮点击、AX press 或 Cmd+1 / Cmd+2 / Cmd+3 都回到同一 `on_approve` 并再次复核 `can_approve` → `tool_approve{ run_id, tool_call_id, decision }`，decision ∈ `approve_once` / `approve_for_run` / `deny`；发出后关闭旧菜单并聚焦 Composer，避免审批卡卸载后焦点悬挂。
3. 清卡路径：对应 `ToolCompleted`、run 终态、历史 `ApprovalResponded`。无任何默认放行：不操作则永远 pending，断线时按钮禁用。

### 4.4 菜单开合与键盘激活语义

- 六种浮层（`MenuKind`：Grouping / Scope / Model / Entry(event_id) / WorkspaceConfirm / Activity）共用单一 `Option<MenuKind>` 状态位：开新即关旧、至多一个打开。
- 行级键盘激活（R3 Wave B Slice 4，design §3.6；Slice 5 P2b 扩展到 rail 聚焦 Button）：聚焦的 ListRow / Button 上裸 Enter / Space 在行级 `on_key_down` 直接调用与鼠标 click 同一激活 handler（task 行 `on_session_clicked`、项目头 `on_toggle_project`、grouping/scope/add-task/项目定向「+」按钮开菜单 / 新建），不走合成 click 兜底——GPUI 对聚焦 stateful 元素的 keyboard click 挂在 keyup 合成路径，真窗口注入取证不可达（[u2-nav slice3 enter-gap](../../ui-review/r3-wave-b/u2-nav/slice3/enter-gap.json)，Slice 4 修复后 enter_gap=0）。物理键盘下该合成 click 仍会到达：`pending_row_key_activate`（行）/ `pending_button_key_activate`（按钮）衔接标记吞除——Slice 5 起按「无按下位置 + 有未消费标记即吞」判定，行键 / 按钮 id 不匹配同样吞除（防跨行 / 跨元素误触发），鼠标真实 click（有按下位置）永不吞（防布尔反转回归）。带修饰键不接管；菜单打开时行级 / 按钮级让位（不 stop_propagation），Enter 由根节点菜单接管。
- Grouping / Scope / Model 菜单的键盘语义（R3 Wave B，design §3.6；Slice 5 修订）：菜单打开即接管（不再要求其触发器聚焦——Tab 移焦 / 外点后键盘仍归菜单），根节点 `on_key_down` 承接 ↑/↓ 移动键盘高亮（`menu_highlight`，wrap；None 时回落当前选中项）、Enter 走与点击同一 select 路径（`activate_menu_item`）、Escape 关闭并把焦点送回触发器；rail ↑/↓、行级与按钮级 Enter/Space 在菜单打开时让位（不 stop_propagation）保证冒泡到根节点。`pending_keyboard_menu_select` 衔接标记吞掉 Enter 选择后触发器同键 keyup 的合成 click，防止「选择即重开」（与外点衔接标记同构）。菜单任何关闭路径复位高亮。
- 关闭路径：选择选项 / 再点触发器 / Escape（根节点 `on_key_down` 冒泡承接；面板经 `deferred` 绘制不可聚焦，组件层不可达）/ 外点（`MenuPanel::dismiss_on_outside` 的 `on_mouse_down_out`）。
- 外点关闭先于触发器 click 到达时，以 `(MenuKind, 按下位置)` 衔接标记判定「同一次物理点击」——位置精确相等才视为关闭收尾不重开；键盘触发无位置永不误判。
- 面板 `occlude()` 拦截下层点击与滚轮（无穿透）；超高在 240px 内自滚。
- 归一化：model 菜单在 `can_switch_model` 翻假期间由 render 关闭；`open_session` 在任何 timeline/session reset 前先关旧菜单（含 Entry 锚点可能被虚拟化卸载的路径）；Inspector 程序化展开前关闭悬浮菜单（防 ActivityPopover 叠面板）。

### 4.5 Fork 与分支切换

1. 条目「···」→ Fork。渲染层 gate：Connected + active session + `entry.is_fork_boundary()`；`on_fork` 入口再复核同三条件（双重防线）。
2. `session_fork{ session_id, parent_event_id }` 被入口 gate 接受后立即聚焦 Composer；响应 Data 提示 `session_id|branch_id`，否则重取 snapshot 挑 `updated_at_ms` 最新 → `SessionForked` → `open_session` 切入分支。
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
- **Composer 草稿与空输入**：per-session HashMap + 无 session 槽；切换 session 先 stash 再 restore（`reset_text`，终端不参与）；`MessageSent` 成功清该 session 草稿，断线保留。空/纯空白输入使 Send disabled（tooltip「Message is empty.」），消除空点击面。
- **禁动符号**（R8 冻结面，bin 内测试钉住内容）：`APP_VIEW_KEYBINDINGS`、`install_keybindings`、`MAIN_PATH_TAB_STOP_IDS`、`resolve_new_task_workspace`。
- **依赖边界**：生产 `pawork-*` == `{pawork-client}`（deny-list 测试）；`projection.rs` 零 gpui / tokio / OS import；协议类型只经 client re-export。
- **时间线单一 reducer**：条目去重 / 合并 / 锚点 / resume 基线语义全部委托 `pawork_client::projection`（protocol 共享 reducer，`TimelineEntry` / `TimelineEntryKind` 直接 re-export）；本包只保留 UI 态与渲染分组。timeline 任何变化统一 `reset(count)`，禁 splice。
- **用户消息乐观回显（R4 Wave B）**：wire 对 `MessageCommitted` 返回 None（用户消息不进实时流），`MessageSent` 回执即经 `note_user_echo` 在 active session 直接 push 一条 UserMessage 行——event_id `local-echo-{run_id}`、timestamp 取 UI 注入的 now 毫秒串、sequence 借用当前最大 wire sequence（不进 `seen`、不占号段，后续 wire 事件严格更大自然落在其后）；非 active session（发送后已切走）不 echo，重放会补；重选 / 重连后 `select_session` / 快照重建 timeline 基线，回显行由持久化 evt- 行替换。禁止为此改 protocol 共享 reducer 或新增 wire 变体。早死路径（plan 闸门拒绝）的宿主合成 `RunChanged{Failed}` 携带 ≥2^60 合成序号（app 侧 `SYNTHETIC_SEQUENCE_BASE`），有序插入落在回显行之后而非时间线顶端（R4 Wave B 评审 P2 修复，投影级回归 `synthetic_terminal_after_user_echo_lands_at_bottom`）。
- **Timeline Top 对齐四合同（R4 Wave A，F-06）**：短会话从 Header 下开始不再沉底；跟随态由 `timeline_following` 单一表达，新内容只在用户贴底时追加跟随；脱钩检测走滚动事件事实——`visible_range` 覆盖末项即贴底、末项滚出即脱钩（Top 对齐下 is_scrolled 滚动过即恒真不可用；handler 内读 ListState 会在 gpui scroll() 写借用存活期重入 panic，且未测高项使像素 max 系统性低估，评审 P0/P1 修复），上滚脱钩不抢滚动、BackToBottom 重挂；条目变化仍统一 `reset(count)` 禁 splice，脱钩恢复钳制偏移。可读列 `TIMELINE_READABLE_WIDTH`=618 左对齐，正文长行必须 wrap。
- **Workspace Header 诚实口径（R4 Wave A，F-05）**：骨架常存，缺字段只隐藏该项；branch 仅 GitDiffInfo.branch、有 active session 且无 session_mismatch 时显示（wire WorkspaceSummary 无 branch）；终态只画 live 可派生 Running / Needs input / Blocked（SessionLiveStatus 同源），wire 无终态字段不画 Completed 绿点；assistant 角色词 render 与 AX 统一为 Pawork；tool 行无耗时字段（wire 无 duration）；Run 摘要卡状态圆随终态种类（Completed 绿 ✓ / Failed danger ✕ / Cancelled —），不对失败/取消宣称成功；Review changes 走真实 Inspector/Changes 入口（先快照可用性再 refresh，Changes unavailable 时 disabled 给原因、Fetching 进行中不误报），Open in editor 无 capability 不画；消息/错误/页脚时间戳经 `display_time` 渲染为相对词 now/Nm/Nh/Nd（epoch millis 解析失败原样兜底，不伪造）。
- **重连三态可见**：Replay / SnapshotRequired / UpToDate 必须以文字在侧栏区分（不只靠颜色）；仅 SnapshotRequired 换基线重分页。
- **TaskRail 状态点诚实语义（R3 Wave A + Wave B）**：`SessionLiveStatus` 三态——NeedsInput（该 session 有待审批）> Running（`active_runs` 成员，含 live `RunChanged` 非终态登记）> Blocked（R3 Wave B live 派生：该 session 最近一条 `RunChanged` 为终态且 state ∈ failed / interrupted，completed / cancelled 不算；同 session 任何其它 `RunChanged` 清除；快照重建清空——wire 无终态来源，Replay 重放终态事件可重新派生）；其余会话一律空心灰圆，wire 无每会话终态字段故不画终态绿点；apply_event 在 active-session 闸门前跨会话维护成员关系，终态按 run_id 移除并清 pendings。unread 为独立通道（`session_unread()`）：非 active session 的 Session-stream 活动事件（RunChanged / AssistantDelta / ToolStarted / ToolOutput / ToolCompleted / MessageSent / Diagnostic；MessageSent 为本地 composer 回执只属 active session，不经 wire）标记，`select_session` 清除，首连 / 快照重建不产生（无 last-seen 基线）。
- **诚实显示**：tokens / quota / tok/s 无权威来源一律 `—`；ContextMeter 只用 catalog 的 `context_window_tokens`；Changes / Resources 未拉取显 unavailable 而非 0；`now_ms` 由 UI 注入，投影层不读系统时钟。
- **终端约束**：`terminal_create` 的 cwd 只接受 workspace 相对路径（拒绝绝对路径、Windows 盘符前缀、`..` 分量）；终端面为滚动文本，无本地 PTY。输入草稿和 create 失败按 workspace 隔离，write 回执只清对应 terminal/草稿；snapshot 的 exited/killed 比 Replay 历史输出更权威。冻结 wire 无 stop/close 命令与 live exit/failure 事件，因此不显示伪 Stop，也不宣称完整终端生命周期。
- **Changes scope**：Host 的 `diff_list_files` / `diff_get` 均解析 latest session；UI 明示该 scope，且两次请求的 session id 不一致时 fail-closed 要求刷新，不能把新会话内容挂到旧列表。
- **心跳配比**：15 空闲 tick（≈15s）对 host 30s 超时的节拍不可静默改动；断线不取消 Run。
- **窗口与焦点**：默认 1440×1024、最小 1080×720（`WINDOW_MIN_SIZE`，R2 Wave B 设计响应式底线——再窄击穿 Workspace ≥560 合同）；macOS 透明 titlebar（无白带、无重复标题栏，R2 Wave A）；启动与用户发起的任务切换、审批、Fork 后聚焦 Composer；激活当前 task 仍关闭菜单并聚焦 Composer，单可见 task cycling 不重开 session 但仍交接焦点；Review changes 展开 Inspector 后聚焦 Changes 选中页签；点击输入框显式拉回焦点（`track_focus` 自动聚焦不够，否则第二轮键盘 / IME / 粘贴进不来）。
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

144 个测试全部内嵌于 bin target（`#[cfg(test)]` 模块；无 crate `tests/` 目录），按文件分布：

| 文件 | 数量 | 覆盖面 |
| --- | --- | --- |
| `main.rs` | 1 | `WINDOW_MIN_SIZE` 钉 1080×720 设计响应式底线（R2 Wave B） |
| `controller.rs` | 12 | 既有 wire/解析/安全回归；R6 Wave B 增 terminal create 失败 workspace 归属与 diff 内容 session id 生命周期字段。 |
| `projection.rs` | 39 | 既有 snapshot/replay、Run/Timeline/TaskRail 与 Terminal 投影；R6 Wave B 增多 workspace terminal 选择、output-before-create 缓存、权威 workspace 回执前不上屏、create 失败占位回收、UpToDate snapshot 合并，以及 exited terminal 不被历史输出复活。 |
| `platform.rs` | 4 | socket / token 默认路径与 instance 命名；socket→token 推导；deny-list 恰为 `{pawork-client}`；扫描器覆盖别名 / target 表（负例含 dev-dependencies 排除） |
| `ui/mod.rs` | 15 | 既有键位、tab_stop、TaskRail 导航、Composer 与 per-session 草稿接线；R6 Wave A Header Activity；R6 Wave B Inspector 键盘目标、terminal gate 与 workspace 草稿/回执归属 |
| `ui/accessibility.rs` | 3 | identifier 唯一与父子关系校验；focus 单一性；bounds hit-test 与无效树拒绝 |
| `ui/accessibility/app.rs` | 6 | 动态 identifier 的转义稳定且无 escape-marker 碰撞；项目 identifier 的日期桶限定与 Projects 模式稳定；Timeline 摘要截尾保持 UTF-8 边界；会话行 AX description 携带状态词（Running / Needs input / Blocked 同源 + 「· Unread」unread 语义，R3 Wave A/B）；Composer AX 面板公式去掉 +68 漂移；R6 Wave A Header Activity 触发器与 Popover/摘要 frame 精确锚点公式 |
| `ui/accessibility/macos.rs` | 6 | 顶左 bounds → AppKit parent space 坐标转换；value-change diff；结构骨架比较（属性变化不触发重建）；settable/action 双门拒绝越权 value / focus 写入；disabled action fail-closed（macOS） |
| `ui/barriers.rs` | 1 | timeline_stable 重写且 settle_seq 单调、字段形状齐全；approval_visible 写入（含 tool 名）与消失删除；未启用（None）零写入 |
| `ui/input_area.rs` | 3 | Composer placeholder 状态机；F-09 footer/model/workspace/context/action 槽结构；Send/Cancel 单槽互换与诚实缺省 |
| `ui/theme.rs` | 8 | 既有 WCAG、TaskRail、Header/Timeline 与 Composer token；R6 Wave A 钉顶层 100×58、二级 96×56、下划线 2、Activity 320×320 |
| `ui/shell_layout.rs` | 4 | R2 Wave A layout invariant：1280 阈值 rail 288↔240 切换；1440×1024 三栏合同（含超长 Workspace 文本仍保持 Inspector 440）；1080×720 Inspector 强制折叠 + Workspace ≥560；resize 折叠与加宽恢复（后三个为 `#[gpui::test]`） |
| `ui/changes.rs` | 7 | ActivityPopover 摘要、二级页签、epoch/path/session 三重拒旧、断线 stale、latest-session mismatch 与真实横滚内容模型 |
| `ui/inspector.rs` | 1 | R6 Wave A 顶层页签默认 Changes |
| `ui/resources.rs` | 2 | 默认 Idle、epoch 拒过期与断线保留旧数据但标记 stale |
| `ui/timeline_entry.rs` | 8 | R4 Wave A 纯逻辑：tool 状态词映射（仅 succeeded→Completed）/ 状态分类 / ToolRowView 构造 / 消息段落与列表切分边界；`display_time` epoch 串 → 相对词（now/Nm/Nh/Nd 边界）与非法串原样兜底 |
| `ui/text_input.rs` | 10 | 多行粘贴行计数；AX set-value 清 marked range；动态 placeholder；Composer 视口预算不破面板总高；Terminal 28–220 独立预算；shift 选择经 SelectLeft/SelectRight 真实 action；IME 经真实 EntityInputHandler 路径 commit 单次入栈且中间态不可 undo；80 行真窗口 overflow scroll（max_offset>0、视口 28–163、caret 滚入视口）；滚动态点击映回可见内容行；reset_text 恢复草稿且清 undo |
| `ui/u1_probe.rs` | 14 | R1 Wave C U1 spike 矩阵 + R5 Wave B SelectAll/Copy/Cut/Undo/Redo、IME commit 单次入栈（真实 EntityInputHandler 路径）、空输入不可发送、Wave B 键位（含 Shift-Enter）keystroke→keymap→action 链路；AX 仍不在本层覆盖 |

**验证命令**：

```bash
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders
```

- `--bins`：本包是 bin-only（无 lib target），任务指南默认的 `--lib --tests` 匹配不到任何 target。
- `--features gpui/runtime_shaders`：gpui 默认构建在编译期调用 Metal 着色器编译器；开发机仅有 Xcode CLT 时缺 Metal Toolchain 会构建失败，runtime_shaders 把着色器编译推迟到运行时使本机可闭环（R8 起的标准口径，历波收口至 R6 Wave B 144/144 绿）。

本包 dev-dependencies 为 `tempfile`（workspace `3`，仅服务 `ui/barriers.rs` 的临时目录测试）与 `gpui` dev 条目（`=0.2.2` + `test-support` feature，R1 Wave C 起；仅测试构建启用 TestAppContext/VisualTestContext，resolver v2 下不进生产二进制闭包），均不计入生产 deny-list。

**运行时验证资产**：`--probe`（连接 + snapshot + 模型目录一行摘要）与 `--probe-smoke`（流式回合 / 切模型 / 审批 / 取消 / 两次断线重连持久化 / `disconnect_survive`），配合隔离实例（`--instance` + `PAWORK_DATA_DIR`）在真实 host 上冒烟。R1 另提供 [scripts/ui-ax-dump.swift](../../../scripts/ui-ax-dump.swift) 做真窗口 AX tree / action 取证；补救前后证据分别见 [ax-gate](../../ui-review/wave-c/ax-gate/) 与 [ax-bridge](../../ui-review/wave-c/ax-bridge/)。端到端流程见 [../flows.md](../flows.md)；验证总策略见 [../README.md](../README.md)。
R3 Wave B 起 [scripts/ui-r3-wave-b-nav.sh](../../../scripts/ui-r3-wave-b-nav.sh) 以真窗口键盘注入驱动导航回归；注入经 [scripts/ui-key-event.swift](../../../scripts/ui-key-event.swift)（CGEvent HID tap，R6 Wave B 增真实横向滚轮事件，R7 Wave C 增纵向滚轮事件）。R5 Wave B 的 [九场景 driver](../../../scripts/ui-r5-wave-b-states.sh) 覆盖 Composer。R6 Wave B 的 [九场景 driver](../../../scripts/ui-r6-wave-b-states.sh) 覆盖 Changes 真 diff/summary/长行横滚、Terminal create/write/output/resize/reconnect/policy 拒绝、Resources 三态、折叠恢复、task/latest-session scope 与断线重连；审查后最终二进制 19 份 AX 断言全绿，证据见 [u2-reviewfix-pass-20260830](../../ui-review/r6-wave-b/u2-reviewfix-pass-20260830/)。审查补修的 terminal 首段串屏与空 diff scope 诊断已由 app/Desktop 定向回归覆盖；第一次补录被 macOS AX 注册 flake 在场景前 fail-closed 的样本保留于 [u2-reviewfix-ax-blocked-20260830](../../ui-review/r6-wave-b/u2-reviewfix-ax-blocked-20260830/)，不构成通过。R7 Wave B 再以导航 26 相位、审批/状态 14 相位及审查边角 3 相位真窗口 U2 钉住任务切换、菜单选择、审批后的唯一焦点，以及当前 task AXPress / 单可见 task cycling；最终 manifest 均为 `structural_pass=true`，首次过期断言与边角 fixture 探索失败也如实保留，见 [r7-wave-b notes](../../ui-review/r7-wave-b/notes.md)。R7 Wave C 的 [耐久 driver](../../../scripts/ui-r7-wave-c-resilience.sh) 在临时派生的 1024 行数据库上覆盖 1080×720 Connected / ActivityPopover / Disconnected、纵向离底/回底、三轮宽窄 resize、焦点与重连，断连相位另接截图级 paint 门禁与千行 `timeline_stable` barrier 硬化；锁屏期 AX 空树阻断与第一次 truncate 修复不足的 FAIL 样本均如实保留，根因定宽修复后最终 15 个 manifest 相位全绿（paint `lit=0`）；证据和单次 `baseline_only` 时延见 [r7-wave-c notes](../../ui-review/r7-wave-c/notes.md)。

## 8. 注意事项与已知限制

- **gpui 前台执行器无 tokio reactor（历史崩溃教训）**：在 `cx.spawn` 的前台执行器上 await client 调用，会在 `receive_frame` 内部的 `tokio::time` 直接 panic（旧 R8 波 A 实证 exit 134，真窗口自始无法启动）。连接期握手 / ack / `subscribe_all` 与事件泵**必须**全部跑在 `runtime.spawn` 上，gpui 侧只经 channel 消费结果。`--probe-smoke` 走 `platform.block_on` 自带 runtime，暴露不了这类回归；R1 已由 [Wave D](../../ui-review/wave-d/notes.md) 建立真窗口启动门禁，R8 继续扩面。
- **Changes 面只读**（用户拍板 2026-08-24）：git_stage / HunkStageService 接线顺延 ADR 候选；`@` 补全浮层与「已加载规则」分区无 Host 出口（`@` 端到端展开在 host 侧 crates/app，不在本 crate）。
- **host `diff_*` 固定解析 latest 会话**：数据会话与当前查看会话不一致时，UI 以 banner「Showing changes for latest session X — not the active session.」与 popover 提示行如实标注，不静默张冠李戴。
- **渲染面自动门禁尚未完整**：R1 已建立 U1 与真窗口 AX/screenshot 来源；R6 Wave B 已把 DiffView 长行真实横滚纳入 U2（offset `-720.0 / 2477.0`），不再只靠历史人工取证。R7 Wave A 的 State A hover/active/focus 九图已由用户签字，并依用户决定以原生 AX tree/action + 纯键盘 + U2 替代本波 VoiceOver；R7 Wave B 的 focus/menu/approval 等价路径已由两套主 U2 与一套审查边角 U2 收口。R7 Wave C 已在默认平台态完成 1080×720、CJK/emoji/超长内容、1024 行虚拟化、反复 resize、断线重连、单次性能基线与连接长文案截图级 paint 门禁。VoiceOver 未执行，屏幕朗读措辞 / 顺序未验证；字号放大与主动 reduced motion / 高对比态仍未完成，性能阈值也未冻结；三张定稿图分区 SSIM 与终局视觉签字留 R8。
- **ActivityPopover 结构已取证，视觉终局未签字**：R6 Wave A 已在 Connected State A/B 三相位通过结构断言，触发器/Popover 锚点与层级成立；分区 SSIM 按 R3–R5 同口径移交 R8，不把结构通过冒充视觉通过。证据见 [r6-wave-a](../../ui-review/r6-wave-a/notes.md)。
- **环境性断连**：显示器休眠 / App Nap 下心跳超时断连（Reconnect 横幅恢复）为宿主环境行为，非缺陷。
- **早死 run 的回显行重选后消失（R4 Wave B 评审 P3，存量语义）**：plan 闸门在 `MessageCommitted` 之前拒绝时，用户消息从未持久化；乐观回显让用户先看见消息，重选 / 重连后快照重建时该行随基线清空消失。消息此前根本不显示，echo 只是使该语义可观察；是否把用户消息持久化提前到闸门之前属产品决策（ROADMAP §5 live wire 诚实缺口仍开放）。同理，合成兜底条目（≥2^60 序号）在屏时若同会话又有真实事件到达，真实事件按序号插到合成条目之前（深边角化妆性排序），重选即自愈。同一 run 的乐观回显行与稍后到达的持久化 UserMessage 在未经重选/重连时理论上可并存（echo 不进 seen）；实际触发面极窄——最新用户消息只经快照到达而快照会重建 timeline——重选即自愈。
- **单主题**：仅深色 `dark()`；`Theme: Global` 是未来运行时主题挂载点，当前未 `set_global`。
- **文件尺寸口径**：`ui/mod.rs` 约 2388 行；这是工程结构口径，不构成新 UI 视觉或交互放行条件。
- **`text_input.rs` 血统**：改自 gpui 0.2.2 `examples/input.rs`（Apache-2.0）。R5 Wave B 已对照上游补齐 Copy / Cut / SelectAll / 拖选 / Undo / Redo / overflow scroll；ShowCharacterPalette 仍裁剪。
- **FollowScroll 的滚轮时序假设**：`on_scroll_wheel` 直读已应用（未钳制）的 offset——依赖 vendored gpui 0.2.2 的 Bubble 相监听逆序分发（内部偏移应用先于用户监听）；升级 gpui 时须重核，做 delta 投影会把增量计两次。
- **UI fixture barrier 钩子（R1 Wave B，测试专用）**：启动读 env `PAWORK_UI_BARRIER_DIR`（main.rs；空值视同未设置，`--probe` / `--probe-smoke` 不发射）。未设置时全程零开销：不 spawn tick、无任何文件 IO。设置后由 ui/mod.rs 既有 1s tick 兼任发射点：已连接 && 无进行中 timeline 分页（`open_session` 置位、complete / `open session` 失败 / `Disconnected` 复位）&& 本 tick 窗口无 ControllerEvent 时重写 `<dir>/timeline_stable`（JSON 含 settle_seq 单调自增 / session_id / entry_count / at_ms / detail）；开始连接、打开会话或收到任一 ControllerEvent 时先删除旧 `timeline_stable` 与 `approval_visible`，防只等存在性的 driver 误收陈旧信号。`pending_approval` 存在且已稳定 → 重写 `approval_visible`（含 tool 名），消失 → 保持删除；目录不存在时由 `BarrierSink::new` 惰性创建。写入 tmp+rename 原子替换、任何 IO 失败静默跳过；`projection.rs` 保持纯状态机零 IO。controller 在未连接、无 TimelinePage 响应或翻页达到上限时发 `OperationFailed`，UI 据此复位分页状态。
