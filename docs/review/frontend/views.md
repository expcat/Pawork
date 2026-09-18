# Desktop 主界面视图层 Review

> GPUI AppView 宿主与工作台主界面装配：连接/事件接线、三栏壳、Timeline 虚拟化、Composer、Inspector 多工具面板、审批卡、快捷查找、Files / Browser 与主题/i18n。范围内 23 个 .rs 文件 / 25 700 行（含测试；不含 components/、settings/、accessibility/ 子目录与 accessibility.rs 模块文件）。事实源为当前工作区源码；与 Spec 冲突处以源码为准并在文中标注。

## 1. 职责与边界

Desktop 四层里，本层是 GPUI view：只负责像素、焦点、键盘、菜单与无障碍投影入口。它不解析 wire、不持有 Host 会话状态机、不直连 Provider / SQLite / 工具 / Git / PTY；文件读写与浏览器动作全部经 DesktopController → Host 命令。

纪律（源码与 docs/spec/crates/desktop.md 一致，本次静态核查通过）：

- 业务副作用只经 DesktopController（pawork-client）；AppView.controller 是唯一出口。ui 顶层文件的 import 面只有 gpui / crate::controller / crate::projection / pawork-browser（browser.rs）/ pawork-terminal 常量（inspector.rs、terminal_view.rs）/ raw_window_handle，无 Provider、DB、Git 直连。
- 展示状态读 DesktopProjection；权威值来自 controller 回执经 projection reducer 收敛，不乐观更新（模型选择、Settings 写、terminal resize 均只 pending）。
- 关闭窗口不取消已进入 Core 的 Run；切到 Settings 路由只换渲染，工作台草稿 / Timeline / Inspector / Run 字段全部保留。
- AppRoute::Settings 下审批 / 取消 / 新建 / Inspector / 任务导航的全局键旁路（workspace_action_active）；字号缩放是应用级键，不受此守卫。
- live 动作共用 live_action_enabled(connection, target_present)：render、键盘、快捷键、AX 最终 handler 入口再复核（fail-closed）。
- FollowScroll 修法仍在：components/follow_scroll.rs 直读 is_scrolled_to_bottom()，无 delta 投影；本层唯一的滚轮消费点（inspector.rs 终端输出）走同一调用。

**Spec 与源码差异（以源码为准，详见主代理回写清单）：**

| 位置 | Spec 说法 | 源码事实 |
| --- | --- | --- |
| spec §4.12 / §4.3 | TIMELINE_READABLE_WIDTH 618、STATUS_BAR_HEIGHT 24 | 880（UI-3 居中阅读列，theme.rs:432）与 30（theme.rs:398） |
| spec §4.3 row_top_gap | 40 / 48 / 48 | MSG_ENTRY_GAP 16 / TOOL_GROUP_TOP_GAP 12 / TIMELINE_FOOTER_GAP 8（Thinking 行同 tool 组） |
| spec §4.4 tool_group_element | 默认展开 | 默认折叠，expanded_timeline_details 记显式展开（timeline.rs:262） |
| spec §4.4 排版 | 段落间隙 28、行高 24 | MSG_PARAGRAPH_GAP 12、MSG_LINE_HEIGHT 26 |
| spec §4.6 | 面板钳制 88–220；MODEL_MENU_* 两常量 | 钳制 110–220（input_area.rs:902）；两常量已不存在 |
| spec §4.8 | stepper 五按钮、plain_terminal_output、三页签 | 2026-09-15 修订已移除 stepper；terminal_view::terminal_screen + render_terminal_lines；InspectorTab 六值（含 Browser / Files / Home 默认） |
| spec §4.4 菜单 | 四种浮层 | MenuKind 八值：+ProjectTask、SettingsRole、SettingsProviderModels、InspectorPanel |
| spec §2 inspector 行 | 工具菜单候选 Changes / 新建 Terminal / Resources | 候选还有 Browser 与 Files（InspectorTab::ALL 五工具） |

## 2. 文件清单

| 相对路径 | 行数 | 职责 |
| --- | ---: | --- |
| apps/desktop/src/ui/mod.rs | 5650 | AppView 宿主：路由、连接、事件分发、焦点/菜单/草稿、Files/Browser 接线、Render 装配 |
| apps/desktop/src/ui/shell_layout.rs | 486 | 壳层几何合同：rail 宽 / InspectorPlacement / InspectorMotion |
| apps/desktop/src/ui/timeline.rs | 1068 | gpui list() 变高虚拟化、行高公式、跟随/脱钩、行组装、回底 |
| apps/desktop/src/ui/timeline_entry.rs | 1898 | 消息/思考/错误/tool group/Run 摘要条目渲染与 Fork、用量页脚 |
| apps/desktop/src/ui/timeline_navigation.rs | 1004 | GUI2-04 已加载正文查找与回合目录、显式阅读模式 |
| apps/desktop/src/ui/approval_card.rs | 166 | pending approval 警示卡与 Allow once / Allow for run / Deny |
| apps/desktop/src/ui/input_area.rs | 1232 | Composer 两行结构、模型菜单、发送/取消同槽、元信息行 |
| apps/desktop/src/ui/text_input.rs | 1998 | 共用单行/多行输入（IME、undo、secure 掩码、overflow scroll） |
| apps/desktop/src/ui/markdown.rs | 1084 | Markdown 子集：表格/代码块/链接/流式 caret；消息菜单动作；parse 结果按文本跨帧缓存 |
| apps/desktop/src/ui/inspector.rs | 966 | Inspector 统一标签栏（PanelTab）、Home 入口、Terminal 页装配 |
| apps/desktop/src/ui/changes.rs | 1274 | Changes Files/Summary、DiffView、ActivityPopover 状态机 |
| apps/desktop/src/ui/resources.rs | 371 | Inspector Resources：MCP 只读清单 |
| apps/desktop/src/ui/task_rail.rs | 1275 | 左侧 TaskRail：scope/分组/项目块/任务行/改名归档/查找入口 |
| apps/desktop/src/ui/quick_search.rs | 956 | GUI2-03 Cmd+K：任务 / 设置行 / Inspector 工具页安全导航 |
| apps/desktop/src/ui/files.rs | 1970 | 右侧 Files：懒加载目录树、Markdown 预览、等宽编辑、草稿与保存 |
| apps/desktop/src/ui/browser.rs | 622 | 任务浏览器面板：地址栏、原生视图桥接、session 保留、250ms 状态轮询 |
| apps/desktop/src/ui/terminal_view.rs | 355 | 终端 gpui 胶水：SGR 着色 runs、光标/预编辑、按键→PTY、尺寸防抖 |
| apps/desktop/src/ui/archive.rs | 274 | UX-08 归档反馈与撤销队列（rail 内 notice + AX） |
| apps/desktop/src/ui/recovery.rs | 399 | UX-04 连接 / 终端原因与恢复入口 |
| apps/desktop/src/ui/theme.rs | 848 | 深色 token、字阶、几何 metrics、motion |
| apps/desktop/src/ui/i18n.rs | 1257 | English / 中文 t() 目录与全局语言态 |
| apps/desktop/src/ui/barriers.rs | 174 | 测试 fixture barrier 文件发射器 |
| apps/desktop/src/ui/u1_probe.rs | 409 | GPUI TestAppContext 探针（不挂 AppView） |

ui/mod.rs 还 mod 了 components / settings / accessibility，本篇不覆盖，见 components-settings.md。

## 3. 界面结构总览

Render for AppView（mod.rs:4766）每帧：同步 browser session / 轮询 / 派发浏览器请求 → 安装 AppKit Tab 监听 → 兑现 pending_scope_focus / pending_inspector_focus → 菜单高亮滚入 → shell_layout::resolve → InspectorMotion 宽度（变化即 timeline_changed）→ sync_accessibility → 按 AppRoute 装配。

宽窗几何：TaskRail 288 / Workspace 弹性（≥560）/ Inspector 440；Timeline 与 Composer 共用 880px 居中阅读列（TIMELINE_READABLE_WIDTH）；StatusBar 仅 Workspace 路由、高 30。窄窗 ≤1279：rail 240，显式打开的 Inspector 改中央呈现（InspectorPlacement::Center）。150% 字号 rail 320，需 ≥1320 才并排。用户 inspector_open 偏好不在 resize 时改写；默认折叠（OPT-4b）。

~~~
┌─ shell-rail (288/240/320) ─┬────────── shell-workspace (flex) ──────────┬─ shell-inspector (440, 可选) ─┐
│ traffic-light 36px 安全区  │ Workspace Header（标题/状态/查找/Activity/展开）│ 统一 PanelTab 栏（工具+文件+PTY） │
│ Tasks + 搜索 + 新建        │ Timeline list() 880px 阅读列，Top+显式跟随     │ Home / Changes / Terminal /     │
│ scope 下拉 + grouping 二态 │ Composer（输入 + footer 动作槽）               │ Resources / Browser / Files     │
│ 日期桶 / 项目头 / 任务行   │───────────────────────────────────────────────┤ （按需懒创建，横向滚动）        │
│ 归档 notice · Local·设置   │ StatusBar 30px（Run 状态与用量，仅 Workspace）  │                                │
└────────────────────────────┴───────────────────────────────────────────────┴────────────────────────────────┘
~~~

Settings 路由：左栏换 Settings Rail，右栏全宽内容；返回工作台不取消 Run。Cmd+K 快捷查找与正文查找为模态浮层，Esc 回焦。

## 4. 组件与方法功能列表

### 4.1 ui/mod.rs — AppView 宿主

- 路由/键位：AppRoute（Workspace/Settings 互斥）；APP_VIEW_KEYBINDINGS（cmd-. 取消、cmd-enter/1/2/3 审批、cmd-n 新建、cmd-i Inspector、cmd-alt 导航、cmd-=/-/0 字号、cmd-k 查找）；workspace_action_active 旁路 Settings 路由的工作台动作。
- 单一菜单位：MenuKind 八值（Scope / ProjectTask / Model / SettingsRole / SettingsProviderModels / Entry(event_id) / Activity / InspectorPanel）；pending_keyboard_menu_select、pending_row/button_key_activate、pending_outside_close 处理键盘合成 click 与外点收尾。
- 关键字段：controller（唯一出口）、projection、text_input/terminal_input、per-session composer_drafts、timeline_list/following/rev、expanded_timeline_details（默认折叠，键 = 首 tool event_id 或 {event_id}:result）、timeline_navigation（查找/阅读态）、quick_search、browser/browser_sessions/browser_request、files（FilesPanel）、inspector_open_tabs、terminal 四类 pending 槽、archive（撤销队列）。
- 连接与事件：start_connect / on_connected / consume_events / handle_controller_event（分发 Snapshot / TimelineLoaded / Event / Terminal* / MessageSent / Settings* / FilesResult / BrowserRequested / Diff* / Mcp* 等）；arm_run_clock 1s tick 刷新用量、写 settle barrier 并在活动 Run 时轮询 poll_browser；emit_settle_barriers。
- 会话/任务：open_session（关菜单、stash 草稿、分页、reset changes）、on_new_session（ADR-054 直建 Unassigned）、行内改名、on_session_archive + undo_session_archive、cycle_active_task / open_next_needs_attention。
- 发送/审批/取消/Inspector：on_send_message（改名/终端聚焦分流，IME 不发）、can_send 空目录 fail-closed、on_approve 三路径同 gate、on_toggle_inspector、panel_tabs 统一标签、refresh_changes/fetch_diff/refresh_resources epoch 拉取、inspector_workspace_id、terminal 谓词组（terminal_can_operate 等，ADR-045）。
- 外观：restore/save_appearance（用户目录 desktop.json）、set_text_scale / set_language；测试构造 persist_appearance=false 不读盘。

### 4.2 shell_layout.rs — 壳层几何

RAIL_NARROW_WIDTH 240 / RAIL_LARGE_TEXT_WIDTH 320 / NARROW_WIDTH_MAX 1279 / WORKSPACE_MIN_WIDTH 560 / TRAFFIC_LIGHT_SAFE_HEIGHT 36 / LARGE_TEXT_INSPECTOR_MIN_WIDTH 1320（=320+440+560）。resolve(width, preferred, large_text) 是 render 与测试唯一计算入口，输出 ShellLayout { rail_width, placement: Hidden/Side/Center, inspector_open }；InspectorMotion 只存瞬时宽度（180ms cubic ease-out，窄窗 snap 归零）；rail_safe_area 36px 无交互占位；ShellProbeHost 复用生产 Panel/StatusBar。7 个测试（1 个 #[test] + 6 个 gpui）。

### 4.3 timeline.rs — 虚拟化容器

四条滚动合同不变（Top 对齐、timeline_following 单一跟随态、脱钩只用事件 visible_range 且 handler 内禁借 ListState、任何变化 reset(new_count) 恢复偏移）。TIMELINE_OVERDRAW=200。行组装 timeline_rows()（消息/思考/错误/工具组/中间相位/终态摘要/live 用量）；tool_group_key 仍取首个 tool event id，tool_group_is_collapsed = 键不在 expanded_timeline_details（默认折叠）；toggle_timeline_detail 翻转集合并脱钩跟随。高度公式：message_entry_height（用户消息 0.8 列宽卡片 + 代码块头槽）、thinking_body_height、run_summary_card_layout（Failed 单行 pill / Completed 标题 + clamp2 说明 / 认证 CTA 行 / Review 右槽）、timeline_row_height、timeline_visible_item_tops / timeline_following_window（AX 可见窗口）、timeline_stack_height。空态：welcome_visible + 离线走连接提示；连接态给 New task / 绑定项目 Ghost 按钮；脱钩且 timeline_content_overflows 才浮出 BackToBottom。timeline_area 同帧复用单份 timeline_rows() 快照（list 渲染闭包与溢出判定共享），条目与审批卡共用 timeline_item_wrapper；消息 parse 经 markdown.rs parse_cached 按文本跨帧缓存（thread_local HashMap，容量 4096 超限整表清空；parse 只读文本，宽度/字号不影响块结构故不入键），render/测高 MessageMeasure/消息菜单/AX 复制共用同一份 Arc（2026-09-18 R-03）。1 个测试。

### 4.4 timeline_entry.rs — 条目渲染

display_time 复用 task_rail::relative_activity（now/Nm/Nh/Nd，解析失败原样）；tool_headline 内建 8 工具按 path/command/pattern 抽目标、HEADLINE_TARGET_MAX_CHARS=80 截断、MCP/畸形回退名称；多工具 tool_group_summary = N tools · 前 3 headline · 状态计数；RESULT_PREVIEW_LINES=10 + {event_id}:result 展开键；ToolRowView::present（from_parts 仅剩兜底）；RunSummaryView / RunSummaryTerminal 区分 Completed/Failed/Cancelled；Failed 紧凑 pill（SUMMARY_FAIL_ICON 16、pill radius 12）+ failure_next_step 认证关键词（401/403/authentication/unauthorized/invalid api key）追加 24px「Open provider settings」；assistant_is_streaming 驱动 Generating；run_metrics_element 用量 chips（入/出/时钟/tok-s）；run_footer_element 终态词 + 时间，不伪造时长；message_entry_element User→浅底卡片 / Assistant→Generating；error_entry_element danger_text 无假 retry；on_fork 入口再核三条件。9 个测试。

### 4.5 approval_card.rs — 审批卡

APPROVAL_CARD_PAD_REMS 0.5 / 按钮行 gap 0.5 / APPROVAL_BUTTON_HEIGHT 32 / 槽宽 [104,116,72]；approval_card_height = pad×2 + 标题 + reason 换行（SM）+ 可选 detail（XS）+ 按钮行；三钮 id approve-once / approve-for-run / approve-deny，decision 字符串冻结；disabled tooltip 走 approve_disabled_reason；app 级 focus handle 卸载不丢；关闭卡不等于允许。

### 4.6 input_area.rs — Composer

880px 居中列（两侧 ≥28、顶 16、底 24）；卡片 16px 内边距/圆角/聚焦描边；composer_panel_height 钳制 110–220（COMPOSER_PANEL_MIN/MAX_HEIGHT）；模型 Raised chip 与 Send/Cancel 同 36px 命中区，触发器上限 220px 截断；菜单向上展开，grouped_model_menu_entries 按 provider 分组扁平（未连接/0 启用整组隐藏），model_catalog_empty_state 是唯一「无已启用模型」空态（connected && loaded && empty）；元信息行：项目 chip / 文件工具不可用 / ContextMeter，窄窗换行；composer_placeholder_hint 只走状态机；on_select_model 只 pending；send/cancel_disabled_reason 诚实 tooltip；IME 组合中 Send 直接 return。6 个测试。

### 4.7 text_input.rs — 共用输入

改编 gpui 0.2.2 examples/input.rs。Composer、Terminal、browser 地址栏、查找输入、quick search、session rename、API key、文件编辑器共用：TextInput（内容/选区/marked/undo 栈/scroll/secure）；sanitize_secure + secure_mask（grapheme 掩码）；set_text 入 undo、reset_text 清 undo；IME commit 单次 push_undo；UTF-16 与 UTF-8 偏移换算；height_clamp；composer_input_max_height；SendMessage action。14 个测试。

### 4.8 inspector.rs — 右栏统一面板

InspectorTab 六值：Changes/Terminal/Resources/Browser/Files + Home（默认，空面板工具入口）；InspectorTab::ALL 五工具驱动「+」菜单与 Cmd+K 面板目标。PanelTab 统一工具页 / 文件（FileTab{workspace_id,path}）/ 同项目多 PTY 标签：渲染、键盘、AX 同一列表，就地关闭、横向滚动。Side（Panel::side_left(440)）与 Center（Panel::fill + inspector-back 返回条）复用同一 inspector_element。Terminal 页：terminal_screen（pawork-terminal 解析 + 非 ASCII 列宽实测）→ render_terminal_lines（SGR runs、行内光标、预编辑下划线）；输出面 canvas 实测像素自动 fit 列×行（80ms 防抖 terminal_resize）；FollowScroll + BackToBottom；输入直通 PTY（无命令草稿、无手动尺寸控件，2026-09-15 修订）；Stop/Close 同槽（ADR-045）。2 个测试。

### 4.9 changes.rs — Diff 视图

数据只来自 diff_list_files / diff_get。ChangesTab Files（默认）/Summary；ChangesPanelState epoch + diff_epoch + stale_reason，断线标 stale 保留最后成功数据；begin_diff_fetch / apply_diff epoch+路径+session 三重拒旧；totals / status_counts / has_reviewable_files_for（CTA 门）/ activity_summary（未就绪显示 unavailable）；changes_status_chip M 琥珀 / A 绿 / D 红、未知单字符原样；parse_hunk_header 与 diff_hunk_line_numbers 行号递推不伪造；DiffView hunk 头 + Add/Delete/Context 着色 + gutter；activity_popover_element 折叠态 320×144；latest-session mismatch banner。8 个测试。

### 4.10 resources.rs — MCP 只读

ResourcesPanelState epoch + available（Settings Tools 导航 gate）+ action_error；apply_authoritative_servers 回执 bump epoch 使在途 mcp_list 失效；mcp_server_name_row / mcp_server_meta_text 与 Settings Tools 共用形状；「已加载规则」无 Host 出口不画。3 个测试。

### 4.11 task_rail.rs — 左侧会话栏

RailView Timeline(日期桶)/Projects(项目块)；relative_activity 相对时间（与 Timeline display_time 共用）；会话状态点仅 Needs input 琥珀 / Running 蓝 / Blocked 红实心，空闲不画（GUI3-06）；连接行 Connected 绿 / Connecting 蓝 / 断线空心灰；grouping 二态按钮图标表达下一步动作（Icon::Grouping / Icon::Clock，tooltip toggle_action_label）；scope 菜单 + 项目折叠 + 定向新建；行内改名、归档立即隐藏；handle_rail_navigation_key RailStop 链；标题截断右侧渐隐；单项目/Unassigned 桶跳过项目头；150% 行高 36px。0 测试（导航与 AX 在 mod.rs / accessibility 覆盖）。

### 4.12 theme.rs — 深色 token

dark() 唯一主题（canvas #141416 / panel #0e0e10 / raised #222226），不读系统偏好；font::TextScale 100/125/150（rem 16/20/24）；字阶 BODY_SM 12 / XS 11 / SM 12 / BASE 14 / BODY 16 / HEADER_TITLE 18 / TITLE 20、MONO=Menlo；metrics 冻结几何：SIDEBAR_WIDTH 288、INSPECTOR_WIDTH 440、STATUS_BAR_HEIGHT 30、TIMELINE_READABLE_WIDTH 880、COMPOSER_SEND_SIZE 36、MSG_LINE_HEIGHT 26、MSG_PARAGRAPH_GAP 12、rail/changes/diff/menu 系列；motion PANEL 180ms / CONTROL 120ms ease-out。9 个测试。

### 4.13 i18n.rs — 界面语言

Language English 默认 / Chinese；LANGUAGES、language_from_identifier fail-closed；t / t2 / session_title / catalog_overview_label，未知 key 原样返回；localize* 纯函数单测；AtomicU8 全局 + AppView 镜像落盘（ADR-053）。不翻译：会话标题、provider/model id、路径、wire 错误。3 个测试。

### 4.14 barriers.rs — 测试 barrier

BarrierSink 读 PAWORK_UI_BARRIER_DIR（None 零 IO）；timeline_stable（settle_seq 单调 / session_id / entry_count / at_ms / detail）与 approval_visible（tool / run_id / at_ms）tmp+rename 原子替换，IO 失败静默；惰性 create_dir_all。1 个测试。

### 4.15 u1_probe.rs — GPUI 探针

#[cfg(test)]；ProbeHost 挂 TextInput + Button + overflow 滚动，不挂 AppView / Platform / socket。覆盖 paste / focus / keystroke / Shift+Enter / 鼠标 / 滚轮 / resize bounds / clipboard / IME 单次 undo / 键位链路。15 个测试。

### 4.16 quick_search.rs — Cmd+K 快捷查找

只搜当前 Snapshot 任务标题 / 项目名 + 设置页 / 设置行（settings_search_entries() 派生）+ Inspector 工具页目标（SearchTarget::Panel，含 Browser 特例）；断线仅 Appearance / Advanced 与本地面板；独立输入、稳定目标选择、Esc 回焦、模态键盘隔离；结果区独立滚动，AX 只发布可见裁剪框；激活走既有 handler（打开任务解除项目筛选 / 展开项目）。2 个 gpui 测试。

### 4.17 timeline_navigation.rs — 正文查找与回合目录

GUI2-04：只查已加载用户/助手正文；MessageKey（Event/Message 双身份）定位；NavigationMode::Find/Turns；reading 显式阅读模式（手动滚动不退出，回底退出）；查找输入独立、Enter/Shift+Enter 切换、sync_navigation_location 与虚拟化 reset 协作；AX 实测布局裁剪。1 个 gpui 测试。

### 4.18 archive.rs — 归档撤销

UX-08：rail 内 notice + 撤销按钮；archive.entries 撤销队列（最后一次优先）；在途/断线 gate（archive_write_enabled）；AX 读实际布局（notice / undo 两节点，enabled 才发布 Press）。1 个测试（mod.rs 侧另有身份/草稿回归）。

### 4.19 recovery.rs — 恢复入口

UX-04：connection_notice_element / terminal_notice_element 就地解释原因；Retry / 诊断详情 / 权限说明；恢复动作不修改 Host 权限；RECOVERY_IDS 六节点 AX 按实际布局裁剪。0 测试（回归在 projection / mod 层）。

### 4.20 files.rs — 右侧文件面板

GUI 1.19：懒加载目录树（逐层展开、prune_stale_dirs 刷新修剪、筛选覆盖已加载层）；FileDocument 独立编辑器 + 草稿（同项目共享、跨项目隔离）；Markdown 预览/源码切换（复用 markdown.rs）；保存回执按 revision 匹配才清 dirty，冲突不覆盖磁盘；关闭确认与窗口关闭 guard（install_files_close_guard）；FilesPanel.workspaces 按 workspace 隔离；标签经共享 PanelTab。2 个 gpui 测试。

### 4.21 browser.rs — 任务浏览器

BrowserPanel：地址输入（Enter 只导航）、后退/前进/停止-重载/Go、状态行；原生 BrowserView 按 canvas bounds + content mask 裁剪，菜单/查找/Settings 时隐藏；sync_browser_session 按 active session 换页保留（HashMap 池）；250ms 状态轮询（ensure_browser_poll）；活动 Run 的 BrowserRequested 经主线程执行 + browser_respond 真实回执（上一条未回执不领取下一条）；切任务明确失败，历史重放不执行。0 测试（AX / 导航在 accessibility/app.rs）。

### 4.22 terminal_view.rs — 终端胶水

terminal_screen（行缓冲解析在 pawork-terminal；非 ASCII 列宽经 shape_line 实测缓存）；render_terminal_lines（SGR 16 色 palette → 主题色 runs、inverse/dim/bold、行内光标块、预编辑下划线插入）；TerminalInput（EntityInputHandler，只缓存 preedit，提交即 Write PTY）；keystroke_to_pty_bytes；TERMINAL_RESIZE_DEBOUNCE 80ms。2 个测试。

## 5. 测试资产（范围内，当前实数）

| 文件 | 数量 | 覆盖面 |
| --- | ---: | --- |
| ui/mod.rs | 19 | 键位、tab_stop、TaskRail 导航、草稿接线、终端按键直通 / fit resize、files/browser 接线 |
| ui/u1_probe.rs | 15 | GPUI 能力地板矩阵 |
| ui/text_input.rs | 14 | 多行 / IME / undo / secure / overflow scroll / 视口预算 |
| ui/timeline_entry.rs | 9 | tool headline / 状态 / 时间 / 流式判定 / 失败分类 |
| ui/theme.rs | 9 | 对比度、TextScale、metrics 基准 |
| ui/changes.rs | 8 | epoch/path/session 拒旧、stale、ActivityPopover、横滚 |
| ui/shell_layout.rs | 7 | resolve 阈值、三栏合同、过渡、150% rail |
| ui/input_area.rs | 6 | placeholder / 面板高度 / 分组 / 空态 |
| ui/resources.rs | 3 | epoch 拒旧、stale 保留、权威回执 |
| ui/i18n.rs | 3 | 双语覆盖、未知 key、id 往返 |
| ui/terminal_view.rs | 2 | 空行 / 组合 runs 完整性、按键转换 |
| ui/quick_search.rs | 2 | 导航布局 / 草稿、断线拒陈旧动作 |
| ui/markdown.rs | 2 | 块结构 / 行数估算、表格 / 链接 / 复制 |
| ui/inspector.rs | 2 | 默认页签、输出序列 |
| ui/files.rs | 2 | 草稿切换 / 保存回执、懒加载树修剪 |
| ui/timeline.rs | 1 | Run 摘要卡高度公式 |
| ui/timeline_navigation.rs | 1 | 查找身份 / 阅读 / 回合 |
| ui/barriers.rs | 1 | barrier 合同 |
| ui/archive.rs | 1 | 归档撤销主路径 |

范围内合计 107 个测试；task_rail / browser / approval_card / recovery 无本文件内测试（由 mod.rs、accessibility 与 projection 层覆盖）。验证命令见包级 Spec §7（cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders；本次为文档任务未运行）。
