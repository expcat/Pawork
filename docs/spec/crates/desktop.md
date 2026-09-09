# pawork-desktop（apps/desktop，二进制）

> 本机单窗口 GPUI Agent 壳（TaskRail + Timeline + Composer + Inspector）：独立进程经 GUI Connection Protocol 连接 `pawork gui serve`，业务依赖**仅** [pawork-client](client.md)；依赖方向为 desktop → client →（re-export）protocol/transport 类型，不被任何包依赖。

## 1. 职责与边界

- **产品定位**：Pawork 桌面工作台。渲染会话列表、时间线、审批卡、变更 / 终端 / MCP 资源面板，把用户操作转成 GUI Connection Protocol 的 Command / Query，把 host 事件流投影成可渲染状态。
- **架构红线**（见 [../../architecture.md](../../architecture.md)）：不嵌入 Core；不直连 Provider、数据库、工具、Git、PTY、quota store；一切能力经 CLI 宿主（`pawork gui serve`）代理。
- **四层结构**（`main.rs` 的模块声明即分层，越界 import 视为违规）：
  - `ui/`：GPUI 渲染与交互。`AppView` 宿主 + 按 Surface 拆分的域模块 + `ui/components/` 基础组件库。
  - `projection/`：纯状态机，**不** import gpui / tokio / OS API；时间线条目语义委托 client re-export 的 `pawork_client::projection` 共享 reducer。
  - `controller/`：唯一业务出口是 `pawork-client`；所有 client 调用跑在 tokio runtime 上，结果经 `smol::channel` 投回 UI 线程。
  - `platform.rs`：socket / token 路径发现 + tokio Runtime 宿主；不触碰 GUI 与业务协议。
- **依赖 deny-list**：生产 `pawork-*` 依赖恰好 `{pawork-client}`。由 `platform.rs` 内测试 `desktop_production_pawork_deps_stay_client_only` 解析本包 `Cargo.toml` 断言，扫描器覆盖 `[dependencies]`、`[target.'cfg(...)'.dependencies]`、`[dependencies.<alias>]` 与 `package = "..."` 重命名形态；dev-dependencies 不计入。
- **能力面**：握手宣告 `Events` / `Snapshots` / `Approvals` / `TerminalStreaming` 四项；**不**宣告 `ArtifactStreaming`（K-08）。
- **断线语义**：断线不取消进行中的 Run（ADR-026）；主动 `disconnect` 只关闭连接，不发 RunCancel。

## 2. 模块与文件地图

63 个 `.rs` 文件，全部在 `[[bin]] pawork-desktop` target 内（无 lib target、无 crate `tests/` 目录）。

| 路径 | 行数 | 承载内容 |
| --- | --- | --- |
| `src/main.rs` | ~670 | 入口与手动 argv 解析（非 clap）；`PAWORK_UI_BARRIER_DIR` env 读取（空值视同未设置，None 全程零开销）；`run_app`（1440×1024 居中窗口 + `WINDOW_MIN_SIZE` 1080×720 最小尺寸——R2 Wave B 设计响应式底线，再窄击穿 Workspace ≥560 合同——+ 沉浸式 titlebar：TitlebarOptions appears_transparent，traffic lights 悬浮深色壳、内容视口贯通全窗，R2 Wave A；`install_keybindings`、聚焦 Composer、安装 macOS AX bridge）；`run_probe` / `run_probe_smoke` 无窗冒烟模式及其 `wait_for_*` 事件等待器；1 个测试 |
| `src/controller/mod.rs` | ~2290 | `DesktopController`（connect / 事件泵 / 独立 15s 心跳任务）；`ControllerEvent`；Command·Query 构造与信封解包后 `serde_json::from_value` 到 protocol Data。`SetApprovalMode.mode` 仍为 String（`ApprovalModeWire::as_str()`）。18 个测试 |
| `src/controller/session.rs` | ~560 | workspace_add / session create（ADR-054：`Option<workspace_id>`，None 直建无归属会话）·rename·archive·fork / run start·cancel / model_list / snapshot·timeline 分页；rename/archive 回执后重取 snapshot；open_session 失败发 `SessionOpenFailed{session_id}` |
| `src/controller/settings.rs` | ~785 | Settings 查询与写（provider_auth_status、auth_*、general/permissions/terminal、mcp_test/remove、set_provider_use_proxy；ADR-055 增 set_model_enabled / set_provider_models_enabled / set_default_role_model（conversation 角色复用既有 default 键，不再经独立 set_default_model），回执 `cleared_roles` 投影为一次性提示文案）；断线不派出 |
| `src/controller/terminal.rs` | ~210 | terminal_create / write / resize / close 与 ADR-045 回执 |
| `src/platform.rs` | ~230 | `Platform`（tokio multi_thread Runtime，`handle()` / `block_on()`）；`default_socket_path` / `socket_path_for_instance` / `token_path_for_instance` / `token_path_for_socket` 路径发现；deny-list 断言；4 个测试 |
| `src/projection/mod.rs` | ~500 | `DesktopProjection` 装配与 live 事件应用；Settings 断线 `mark_settings_stale` 单点扇出 |
| `src/projection/session.rs` | ~710 | `ConnectionState` / `ResumeState` / `PendingApproval` / `ModelEntry` / `ActiveRun` / session·workspace 摘要 / TaskRail 分组；P0-2 将 grouping 当前 `view_label`、目标 `toggle_action_label` 与 `toggled` 分开，避免 AX name/value 混写；`group_models_by_provider` |
| `src/projection/settings.rs` | ~710 | `SettingsQueryGate`（loading/stale/available/writes_enabled）与各 Settings 页 wrapper；Host Data 走 `pawork_client` protocol 类型 `from_value` fail-closed（缺 nullable 键不算未设置）；`parse_auth_change`（AuthChanged 非 CLN-4 Data）；`ProviderStatusLabels::auth_label` 只返回连接态，不拼接 masked credential 或错误详情；`confirm_use_proxy` 落地 ADR-052 `set_provider_use_proxy` 写回执（回执即写后状态，不另重查）；ADR-055 增 `SettingsRole` 四角色（conversation/naming/vision/search）、`RoleDefaultsState` 与角色键对确认、`model_catalog` 启用集投影（确认回执改 enabled，不整表重拉）、`model_cleared_note` 清除反馈 |
| `src/projection/terminal.rs` | ~470 | `TerminalState` / 多 workspace 终端 / live exit / Close 清理 / 新建终端初始尺寸；UX-04 `last_error` 保存操作错误，运行态不因 I/O 失败丢失，成功写入 / 尺寸确认后清除 |
| `src/projection/timeline.rs` | ~200 | Timeline 行分组与 Run footer/summary 文案 |
| `src/projection/tests.rs` | ~3230 | 67 个投影测试（snapshot/replay、Run/Timeline/Terminal、Settings typed 解析 fail-closed） |
| `src/ui/mod.rs` | ~4593 | `AppView` 宿主：Workspace Header、Timeline、Composer 与 Inspector 三栏装配；SET-3 增顶层路由 `AppRoute`（Settings 壳与工作台互斥渲染，工作台状态保存在字段、返回即恢复；进入拉取 provider 状态并断线 mark_stale）；SET-4 增 Settings 写操作宿主字段（secure 输入实体 / Replace 编辑器 / Remove 确认 / 动作焦点）、`AuthStarted` 消费、Succeeded 后再查 provider 状态、auth_start 失败回滚乐观态，离开 Settings 清空 secure 缓冲（含 undo 栈）；SET-5 增 `DefaultModelConfirmed` 消费（Composer 同步已确认默认）、页级 Refresh 入口（重查 provider_auth_status + model_list，失败保留旧列表并显示错误）、进入 Settings 即补拉模型目录（与 Refresh 对称、断线 no-op）与 ModelsLoaded 后「设为默认」按钮焦点回收；SET-6a 增 `SettingsPage`（Network 仅在 `general_settings` 查询成功后可选；内部 enum/wire 保留 General 兼容名）、进入/Refresh 同拉 `general_settings`、断线 `mark_stale`、`GeneralSettingsLoaded` / `ProxyUrlConfirmed` 以回执为权威生效值（迟到响应仍重标 stale）；SET-6b 增 `SettingsPage::Permissions`（查询成功后可选）、进入/Refresh/重连同拉 `permissions_settings`、断线 mark_stale、`PermissionsSettingsLoaded` / `ApprovalModeConfirmed` / `WorkspaceTrustConfirmed` 以回执为权威生效值（迟到响应仍重标 stale）；SET-6c 增 `SettingsPage::Tools`（`mcp_list` 成功后可选，可用性=resources.available）、进入/Refresh/重连同拉 `refresh_resources`、`McpServersReceipt` 以回执为权威生效值、Remove 两步确认 `settings_mcp_remove_confirm`（离开 Settings 清空）；SET-6d 增 `SettingsPage::Terminal`（`terminal_settings` 查询成功后可选）、进入/Refresh/重连同拉 `terminal_settings`（重连同批预热新建终端初始尺寸查询缓存）、断线 mark_stale、`TerminalSettingsLoaded` / `TerminalSettingsConfirmed` 以回执为权威生效值并回填输入框（迟到响应仍重标 stale）、`TerminalCreated` 回执后投影初始尺寸与当次 `terminal_resize` 改用生效 columns/rows（未查询到回落 80×24）；SET-6e 增 `SettingsPage::Appearance`（本地外观页常在，导航焦点 + 三档字号 HashMap 焦点）；SET-6f 增 `SettingsPage::Advanced`、握手摘要与导航焦点；Connecting/断线清空摘要，连接成功刷新，避免旧 Host 信息冒充当前状态；Scope 菜单的 `Add project…` 通过 GPUI 系统目录选择器调用 controller `open_workspace`，成功后切换 scope、同步 terminal 并显示项目名；其余承载 Activity、按 workspace 隔离的 terminal 草稿、尺寸草稿/键盘 stepper、终端生命周期、焦点、字号、五种浮层菜单与 barrier settle；P0-2 移除 `MenuKind::Grouping` 分派；OPT-2a 移除 `MenuKind::WorkspaceConfirm`（ADR-054）；P0-3 让 Header 以 subtle divider 收口，并在无 Task 时隐藏重复 Header 新建动作；OPT-4b（F6）：Inspector 初始默认折叠（显式动作仍可展开），折叠态 Header 最右新增 inspector-expand 重开按钮（40×37 槽、font::ICON 20px 字形，与 Activity 触发器并存）；OPT-4a：header-new-task / inspector-toggle 字形提至 20px；17 个测试 |
| `src/ui/recovery.rs` | ~380 | UX-04 连接 / 终端原因与恢复入口、创建 gate、技术详情、按实际布局裁剪的 AX；恢复动作不修改 Host 权限 |
| `src/ui/shell_layout.rs` | ~295 | R2 Wave A 壳层几何合同：`resolve`（唯一计算入口，render 与 AX 树共享）——宽窗 rail=288 / Inspector 打开时 440（OPT-4b 起 AppView 初始偏好为折叠，resolve 合同不变），窗口宽 ≤1279 时 rail=240 且 Inspector 强制折叠（Workspace ≥560）；R7 Wave C 在 150% 字号下改用 320px rail，宽度不足 1320 时保持 Inspector 折叠，1080 窗口保留 760px Workspace；固定侧栏 `flex_none`，防长文本 min-content 挤窄 Inspector；rail 顶部 36px traffic-light 安全区；4 个 GPUI 布局测试 |
| `src/ui/accessibility.rs` | ~410 | 平台无关 `AxTree` / `AxNode` / role / action / request / rect 模型；声明 Settings AX 子模块；3 个测试 |
| `src/ui/accessibility/app.rs` | ~4966 | 工作台三栏语义树与 Press 白名单（含 Settings identifier 派发）；P0-2 的 `task-rail-grouping` 发布目标动作 name + 当前视图 value，Press 直接切换，不再发布 expanded/menu child；P0-3 空态发布 title / description / 单一 New task action，disabled 时不发布 Press；P1 的 tool group / Review 与 Activity 几何、P2 的 credential summary 隐藏和 Settings 写 gate 与 render 同源；UX-05 model 菜单使用实际布局 / 内部列表滚动框裁剪 AX，打开定位当前项，搜索与键盘高亮共用筛选结果；Settings 页树拆到 `settings_*.rs`；OPT-3 增 Manage models 弹层（触发器 / 单模型 Switch / 全开全关 / 空目录）与四默认角色触发器·菜单的 identifier 派发；ADR-056 增展开 chevron（Press、value Collapsed/Expanded）与展开区 Credentials 凭证行 / Usage 空态节点；UI-2 会话行动作与项目头计数 AX 几何跟 render 同源；20 个测试 |
| `src/ui/accessibility/settings.rs` | ~366 | Settings rail / 页分发；内容列宽经 `SETTINGS_CONTENT_PAD`（32×2）与 render 全宽列同源（OPT-4c 取消 820 钳制）；OPT-4d 增导航选中/未选中两态 AX frame 逐像素一致断言（F4 零位移） |
| `src/ui/accessibility/settings_providers.rs` | ~850 | Provider 64px 概览 AX；连接、目录、详情分层，普通 group value 不含 credential、endpoint、错误或 raw model id；列几何经 `settings_content_ax_width` 与 render 全宽内容列（`SETTINGS_CONTENT_PAD` 32×2，OPT-4c）同源（auth-methods 并入 name value 后 connection/catalog 平移至 +300/+440）；enabled 动作才发布 Press；OPT-3 增「Default models」四角色区（触发器 value 为当前对或「未设置」，候选 = 已连接 provider 的已启用模型，vision/search 标注只保存不接路由）与 Manage models 弹层 AX（Switch 状态词 On/Off、Enable all/Disable all、空目录 fail-closed 文案） |
| `src/ui/accessibility/settings_general.rs` | ~150 | Network 页 AX（wire/identifier 保持 General 兼容名） |
| `src/ui/accessibility/settings_permissions.rs` | ~220 | Approvals 整行 radio AX（identifier `settings-approval-mode-{wire}` 不漂；selected/enabled/Press 与 render 同源） |
| `src/ui/accessibility/settings_tools.rs` | ~150 | MCP 页 AX |
| `src/ui/accessibility/settings_terminal.rs` | ~220 | 终端页 AX |
| `src/ui/accessibility/settings_appearance.rs` | ~180 | 外观页 AX（含字号与语言切换节点，name / value / selected 与 render 同源） |
| `src/ui/accessibility/settings_advanced.rs` | ~80 | 高级页 AX |
| `src/ui/accessibility/settings_about.rs` | ~45 | 关于页 AX |
| `src/ui/accessibility/macos.rs` | ~940 | ADR-042 AppKit bridge：`GPUIView` AX root、`NSAccessibilityElement` 虚拟元素、frame / parent / hit-test / focus / notification / retain-release、settable/action 双门与 action 回调；结构不变（identifier/role/press 能力/子树形状）时原位刷新既有 element 而非整树重建，内部树同步 super 直调不触发 action 回调；6 个 macOS 测试 |
| `src/ui/barriers.rs` | ~175 | UI fixture barrier 发射器（R1 Wave B）：`BarrierSink` 读 `PAWORK_UI_BARRIER_DIR`（None 零开销直通）；`timeline_stable`（settle_seq 单调自增 / session_id / entry_count）重写与 `approval_visible` 写/删；tmp+rename 原子替换、IO 失败静默；1 个测试 |
| `src/ui/theme.rs` | ~694 | 深色单主题 token + 以 16px 根字号表达的 `Rems` 字阶 + 100%/125%/150% `TextScale` + `metrics` 尺寸常量；P0-1 收敛为 Header 22、title 20、正文 16、control 14、meta 12px，并冻结 4/8/12/16/24/32 spacing、4/6/8 radius、2px focus、36px icon button 命中区（OPT-4a；可见字形经 `font::ICON` 20px）、220–360px menu；surface 含 pressed 色；单一 dark palette，不读取系统显示偏好；9 个定向测试 |
| `src/ui/i18n.rs` | ~850 | 界面文案目录（English / 中文）与全局语言态：`Language`（默认 English）、`AtomicU8` 当前语言、`t(key)` / `t2(key, a, b)`（未知 key 原样返回）、`set_language`、`catalog_overview_label` 与纯函数 `localize*` 测试面；3 个定向测试 |
| `src/ui/timeline.rs` | ~730 | Timeline 容器：gpui `list()` 变高虚拟化与显式跟随；`timeline_rows()` 同源组装五类行，按既有 run/order 把连续 tool 聚合并由 terminal summary 吸收重复相位；P1 以首个 tool event id 作为稳定折叠 key，live / replay 共用结构，折叠态进入共享行高 / 可见窗口 / AX 公式；880px 居中阅读列、短会话 Top 对齐、空态唯一 New task 与 `TIMELINE_OVERDRAW`=200px 合同不变 |
| `src/ui/timeline_entry.rs` | ~830 | 消息、tool group、Run summary/footer 与 error 的工作单元呈现；P1 tool group header 汇总 `N tools · <state counts>`，默认折叠，mouse / Enter / Space / AX 共用折叠状态；状态与 detail 不伪造 wire 缺失耗时。Run summary 按 Completed / Failed / Cancelled 区分，只有当前 Session 存在至少一个真实、可审阅的 Changes 文件时才显示唯一主 CTA `Review changes` 并聚焦 Changes；Open in editor 无 capability 不画；6 个测试 |
| `src/ui/markdown.rs` | ~720 | UI-3 / UX-02 Markdown 子集：管线表格、保留原换行的代码复制、HTTP(S) 链接打开 / 复制；`message_actions` 供消息菜单共享，render / 行数估算共用 parsed blocks；2 个定向测试 |
| `src/ui/approval_card.rs` | ~160 | 审批卡：警示卡 + Allow once / Allow for run / Deny 三按钮（P4 片 3 按钮 32px 槽位，卡高 `approval_card_height` 公式与 AX 同源：p_2 + 标题/reason（+可选 detail）行数 + 按钮行）；app 级 focus handle（虚拟化卸载不丢失）；禁用原因 tooltip；R7 Wave B 的 mouse / keyboard / AX 三路径汇入同一 gate，决策后关闭旧菜单并把焦点交回 Composer |
| `src/ui/input_area.rs` | ~760 | UI-4 Composer：880px 居中列，两侧至少 28px、顶部 16px、底部 24px；输入卡片 16px 内边距、12px 输入 / 动作间距、110–220px 高度预算。模型 Ghost 按钮与 Send/Cancel 同为 36px 命中区，模型固定 220px 槽、长名截断，菜单继续向上展开并按 provider 分组。卡片外显示只读项目 chip 与 ContextMeter，UX-03 将无项目文件工具提示与项目新建入口并入元信息行，窄窗随宽度换行，status_hint 独立；render / AX 共用实测布局与说明。per-session 草稿、IME、发送 / 取消与空模型 gate 沿用；不新增附件或队列能力。4 个定向测试；实际 GPUI 布局与 AX 操作区对照位于 accessibility/app.rs |
| `src/ui/inspector.rs` | ~730 | Inspector 面板：顶层 Changes / Terminal / Resources 三页签；OPT-4a 起面板内 `inspector-collapse` 折叠按钮为 36×36 命中区 + 20px 字形；Terminal 页含 cwd/尺寸 stepper 组（G1：本地草稿 ±步进、apply 走冻结 `terminal_resize`；P4 片 3 五按钮冻结 28/28/72/28/28×28 槽位，头部 px_2/py_1/gap_1 以共享 rem 常量表达并与 AX `terminal_stepper_ax_rects` / `terminal_header_height` 同源）、FollowScroll 输出、输入与 Start/New/Size 单槽（G2：已知 exited/killed 终端 Start 变「新建终端」入口）、ADR-045 Stop/Close 同槽按钮（running→Stop 真实 `terminal_close` 终止，已知 exited/killed/failed→Close 清理 Host tombstone；failed 不直接 New，在途禁用防连点）；`plain_terminal_output` 在可见文本与 AX 共用路径移除 ANSI/VT 控制序列并归一换行（纯文本视图，不冒充 VT emulator）；`ensure_terminal` 懒创建与 exited/killed 重建共用 `begin_terminal_create`；3 个测试 |
| `src/ui/changes.rs` | ~990 | Changes 面：Files / Summary、只读 DiffView、latest-session mismatch fail-closed 与诚实 scope；empty / unavailable / stale 使用分层占位。`has_reviewable_files_for` 要求 active session、Ready、非 stale 且文件非空，供 Run summary render/AX 同源门控。折叠态 Header ActivityPopover 为 320px 内容宽，内容高随字号为 144/180/216px，按当前唯一 Changes 内容收缩，不为未实现的 Agent 状态留空；7 个测试 |
| `src/ui/resources.rs` | ~290 | Resources 页：MCP server 只读表 + `ResourcesPanelState`（epoch 防过期）+ 手动刷新；empty / unavailable / error / stale 分层；SET-6c 权威回执 bump epoch；3 个测试 |
| `src/ui/settings/mod.rs` | ~1386 | English / 中文 Settings 壳与稳定八页导航；OPT-4c 起内容列用满 Rail 外可用宽度（`SETTINGS_CONTENT_PAD` 32×2，不再保留 820px 上限），共享脚手架单列 + 滚动层，受限高度内纵向滚动、切页归零；OPT-4d 导航选中/未选中共用同一外壳几何（UI-5：中性背景 + 字重；焦点描边经 focus_ring 覆盖层，零布局参与），文字坐标零位移（F4）；共享状态 / gate、descriptor 驱动认证动作与 Connect API key 编辑入口；4 个测试（空 shell Save 映射 null；OPT-3 角色区/弹层渲染 gate；角色控件 identifier 构造/解析互逆） |
| `src/ui/settings/provider_quota.rs` | — | Go 账号三窗已用 / 剩余、天 / 小时 / 分钟倒计时、来源 / 获取时间与过期标注；刷新、选择模式控件与共享 AX 状态（ADR-060 / UX-06） |
| `src/ui/settings/providers.rs` | ~1620 | 64px provider 概览（认证方式 / 连接 / 目录或模型数）+ 独立认证操作详情行；普通层不显示 masked credential、endpoint、catalog error 或 raw model id；供应商列表后的「Default models」四角色区取代独立默认模型 section；Remove 保持二次确认；ADR-056 起卡片改为「64px 五列卡头 + chevron 展开区」（展开态按 provider_id 存 projection，有效展开 = 显式态 ∨ 流程态）：UX-06 展开区优先展示 Accounts（旧 Host 为 Credentials）区，其后是 Proxy / Manage models；账号区（每条存储凭证一行：kind + masked + Connected/Expired，空列表诚实空态，env fallback 不入列）与 Usage 行（Go 由 ADR-060 按账号展示三窗已用百分比与重置；其它来源不可用时保留 unavailable）；展开区文本列一律 flex_row + flex_1 骨架（region 根 min_w_0 会以 Definite(0) 测量毒化 auto-basis 裸 flex_col 文本，真窗口实证）；ADR-052 provider 级代理 Switch：仅 Global `proxy_url` 已配置时可见，按生效值并排 On/Off 状态词，writes 总闸关闭（断线 stale）时 disabled；mouse / Enter / Space / AX Press 共用 `on_settings_toggle_provider_use_proxy`（OPT-3c 起控件形态为 `Switch`）；ADR-055 增供应商列表后的「Default models」四角色区（角色菜单候选 = 已连接且已启用，含 Clear；候选为空仍保留 Clear（鼠标/键盘/AX 同源）；vision/search 标注 save-only）与每 provider「Manage models」弹层（`MenuPanel` 内单模型 Switch 行 + Enable all/Disable all，禁用命中角色默认对时显示 cleared_roles 提示；目录为空显示诚实空态，不渲染全开假按钮） |
| `src/ui/settings/general.rs` | ~220 | Network / HTTP proxy（wire 保持 General 兼容名） |
| `src/ui/settings/permissions.rs` | ~340 | 五档整行 radio + 会话信任；row click / Enter / Space / AX Press 共用 handler；`ApprovalModeWire` English 标签在 `approval_labels.rs` |
| `src/ui/settings/tools.rs` | ~270 | MCP list/test/remove |
| `src/ui/settings/terminal.rs` | ~260 | 终端默认值 |
| `src/ui/settings/appearance.rs` | ~260 | 本地三档字号 + 随当前字号即时变化的正文 / control 样例 surface + English / 中文语言切换（同源按钮，切换走 `AppView::set_language`） |
| `src/ui/settings/advanced.rs` | ~200 | 连接诊断 definition list，长路径正常换行与纵向滚动 |
| `src/ui/settings/about.rs` | ~130 | Host data directory 只读 definition list |
| `src/ui/settings/approval_labels.rs` | ~30 | 五档 English label/description |
| `src/ui/task_rail.rs` | ~1133 | Sessions 侧栏：顶部三行——20px `Pawork` + 36×36 ghost grouping 直接切换（OPT-4a，字形 20px）、16px 全宽 scope 菜单、12px 连接行 + 36×36 全局「+」；grouping 图标/tooltip 表达目标动作，mouse / Enter / Space 共用 `toggle_grouping`，切换关闭其它浮层并保留 active session、scope、draft 与 collapsed projects；日期/项目/44px task 行、状态点、unread/blocked、Reconnect 与 Local/Settings 语义不变；列表仍用行级 bounds，在 grouping/scope 变化后滚动 active task 到可见；rail 键盘焦点链与 scope 菜单从触发器下方 8px 展开，MenuRow 单勾，AX 同源锚点；UI-2 / ADR-054：任务行右侧改名/归档在悬停、行或动作聚焦、当前会话时露出（命中区 32×32=`RAIL_SESSION_ACTION_SIZE`，与时间戳共用 64px 槽；项目头计数进入同一悬停/焦点面；键盘/AX 可达；改名行内编辑 Enter 提交、Esc 取消、空白不提交） |
| `src/ui/text_input.rs` | ~1890 | `TextInput`（Composer / 终端 / Settings 共用，视口高度上限至少容纳当前字号单行与内边距）：内容 / 动态 placeholder / IME marked_range / UTF-16 映射 / 视口 max_h + overflow scroll（TextElement 按完整内容高布局，视口由父容器 max_h 兑现，caret 滚进视口按 ScrollHandle 容器高计算；鼠标与 IME 坐标映射基于归一化布局原点 content_bounds——prepaint 时 origin 只减输入框自身 ScrollHandle offset，保留祖先页面滚动位移——再减 scroll offset，行高取 paint 时 last_line_height）/ 选择复制剪切 / Undo Redo / reset_text 草稿恢复；UX-01 起 Composer（`TextInput::new`）按可用宽度软换行，GPUI 换行边界对齐完整 grapheme 并以最终显示字宽复核，测高 / 绘制 / 光标 / 选择 / 鼠标 / IME / 滚动共用同一显示行与原文 byte 起点，不改草稿原文；`with_placeholder` 的终端 / Settings 字段、secure 与只读 URL 不启用软换行；OAuth `read_only()` 模式保留选择 / Copy、长行横滚，阻止用户编辑与 IME，Enter 不冒泡发送；SET-010 secure 模式（SET-4）：渲染与 AX value 只发布 grapheme 掩码、Copy/Cut 不写剪贴板、粘贴 / AX set-value / IME 剔除 CR/LF（单行语义）、光标 / 选择 / 鼠标映射经 grapheme 偏移换算；13 个测试 |
| `src/ui/u1_probe.rs` | ~410 | R1 Wave C U1 spike：真实 TextInput/Button/overflow 探针；R5 Wave B 增 SelectAll/Copy/Cut/Undo/Redo、IME commit 单次入栈（真实 EntityInputHandler 路径）与空输入不可发送、Wave B 键位（含 Shift-Enter）经 keystroke→keymap→action 真实链路覆盖；14 个测试 |
| `src/ui/components/mod.rs` | ~13 | 组件族模块声明 |
| `src/ui/components/switch.rs` | ~160 | `Switch`（OPT-3c）：36×20 轨道 + 16px 滑块，On/Off 状态词由调用方并排渲染；disabled 无 Press；mouse / Enter / Space / AX Press 同源 |
| `src/ui/components/button.rs` | ~407 | `Button`：六 variant（Primary / Ghost / Danger / Success / Raised / IconCircle；IconCircle 为 Composer 32×32 圆形动作槽）的底色·文字色·hover/pressed 映射；默认 14px control / `CONTROL_RADIUS`；`child()` 承载自绘图标；焦点描边经 `focus_ring` 覆盖层（零布局参与）；`ButtonPadding` 四档及描边/尺寸/对齐 builder；disabled 无 pointer cursor、mouse press、hover 或 keyboard activate，仍保留可读文字与 tooltip |
| `src/ui/components/dropdown.rs` | ~270 | `Dropdown`（触发器 + `deferred(anchored())` 浮层；可选局部 Corner/Point 锚点，anchored 负责窗口碰撞）、`MenuPanel`（220–360px、8px padding、r6、strong border + menu-only shadow、默认最大高 240px、内部滚动、`occlude()` + 外点关闭）、`MenuRow`（34px；选中用 accent check + raised surface，不用整行亮蓝；hover/pressed/disabled/键盘高亮语义；长 label 单行截断）、`ANCHOR_GAP_Y`=8px |
| `src/ui/components/focus_ring.rs` | ~133 | `focus_ring(radius)` 返回 `FocusRing`：持焦且使用键盘导航时以 `absolute + inset_0` 绘制 2px `text.secondary` 中性描边；鼠标点击不绘制。单窗口输入方式由窗口 mouse capture、根 key capture 与 AppKit Tab 监听器同步；菜单遮挡或子控件停止冒泡仍会清除鼠标路径描边。覆盖层无 hitbox、不 track_focus、不参与布局，圆角与外壳同源；1 个 GPUI 输入切换回归。约定：禁止在 `.focus(...)` 等动态样式里设置 border 等参与布局的属性（gpui border 参与 Taffy 盒模型，聚焦加边框会膨胀控件或压缩内容盒，OPT-4d 微移根因） |
| `src/ui/components/follow_scroll.rs` | ~90 | `FollowScroll`（`ScrollHandle` + 跟随位：贴底判定 / 脱钩 / 重挂；现仅终端使用）与 `BackToBottom` 回底控件容器（绝对定位右下） |
| `src/ui/components/label.rs` | ~70 | `Label`（单行文本，token 化字号 / 颜色）与 `Badge`（状态徽标别名，默认 12px meta + text.secondary） |
| `src/ui/components/list_row.rs` | ~184 | `ListRow`：Task 行与 ProjectHeader 行两形态，默认行高 44 + 垂直居中；`height()` 供两行 Settings 审批项使用 3.5rem（56/70/84px），不改变 TaskRail 默认；`radius()` 覆盖外壳与焦点描边圆角；`min_w_0` 保证子项 truncate 拿到确定宽度；selected/hover/pressed 使用 raised/hover/pressed surface，焦点描边经 `focus_ring` 覆盖层（零布局参与）；裸 Enter / Space 调用与 click 同一激活 handler |
| `src/ui/components/panel.rs` | ~78 | `Panel`：`side_right`（TaskRail，右描边 + gap/p-2）与 `side_left`（Inspector，左描边）固定宽面板壳；固定宽时 `flex_none`，禁止 Workspace 长内容把侧栏挤窄 |
| `src/ui/components/status_bar.rs` | ~70 | `StatusBar`：底部 24px 状态行容器（顶描边 + 12px secondary 文字）；F-13 信息串在最小宽度为零、overflow hidden 的绝对居中槽内裁切；R6 Wave A 起不再承载 Inspector/Activity 动作 |

> **SET-6e 模块增量（2026-09-03）**：`ui/mod.rs` 增 `SettingsPage::Appearance`；外观页现位于 `ui/settings/appearance.rs` 与 `ui/accessibility/settings_appearance.rs`。
>
> **SET-6f 模块增量（2026-09-03）**：`controller` 在 `DesktopConnect` 保留非 Secret `DesktopHandshakeInfo`；高级页现位于 `ui/settings/advanced.rs` 与 `ui/accessibility/settings_advanced.rs`。

> **SET-6g 模块增量（2026-09-03）**：握手 `host_data_dir` 进 `DesktopHandshakeInfo`；关于页现位于 `ui/settings/about.rs` 与 `ui/accessibility/settings_about.rs`。
>
> **CLN-5 模块增量（2026-09-04）**：删除 `projection.rs` / `controller.rs` / `ui/settings.rs` 神文件（无 shim）；Settings JSON 经 `pawork_client` protocol 类型反序列化；`SettingsQueryGate` 统一 loading/stale/写 gate；断线 `refresh_all_settings` + `mark_settings_stale` 单点扇出；AX identifier 不漂。`SetApprovalMode.mode` 仍为 String。

ADR-053 新增 `src/platform/preferences.rs`：标准用户配置目录的 `desktop.json`；std + serde_json、进程内串行 read/modify/write + 同目录临时文件 rename，保存只修改用户操作的单项，另一项沿用磁盘最新值（避免多个实例依次保存时旧快照覆盖），保留未知键；缺文件默认，损坏/权限失败保旧并经 Appearance 的可见提示与 AX 同源展示。`AppView::restore_appearance` 只由正式窗口 bootstrap 在首帧前调用，恢复语言和 window rem；纯 UI 状态构造不访问用户磁盘。无新增业务依赖。`preferences_restore_and_preserve_other_keys` 覆盖两个旧窗口快照依次修改不同字段且互不覆盖；`damaged_preferences_are_not_overwritten` 覆盖损坏文件保护。

> **UI-5 Settings（2026-09-08）**：`ui/settings/mod.rs` 增非供应商页共享分区样式及本页 `settings_element_layouts` 实测框；`ui/mod.rs` 在所有 Settings 页 prepaint 后同步 AX。七个 `ui/settings/{general,permissions,tools,terminal,appearance,advanced,about}.rs` 重排页内层级，对应 `ui/accessibility/settings_*.rs` 删除估算纵坐标，使用实际框并经 `settings_page_ax` 裁掉离屏项。Settings 导航 40px / 14px / r6，按钮 36px / 14px / r6，内容仍全宽、水平 32px，非供应商页垂直留白在 100% 下为 32px 并随字号缩放；详见 [GUI 设计 UI-5](../../gui-design.md#ui-5-设置更新2026-09-08)。增加一个实际布局 / 滚动 / 断线回归；无新模块、依赖、feature 或 wire。

UX-01：Composer（`TextInput::new`）按可用宽度自然换行，GPUI 换行边界对齐完整 grapheme，并以最终显示字宽复核；测高、绘制、光标、选择、鼠标 / IME 定位与滚动共用显示行及原文 byte 起点，软换行不改草稿原文。`with_placeholder` 的终端 / Settings 字段保留既有换行策略，secure 与只读 URL 不开启软换行。IME 组合选区相对当次组合文字计算 UTF-16 偏移；组合更新、宽度与字号变化后光标滚入视口。Composer 仍遵守 220px 卡片上限与逐会话草稿语义；AX 布局优先使用实际输入视口高度。

UX-04：连接失败 / 断线的原因在主区呈现，无任务时替代新任务空态，有任务时保留 Timeline 与草稿；重试显示连接中及重复失败次数，诊断进入 Advanced。模型按钮文案与 AX 同源区分连接状态、目录加载中和无模型。Terminal 操作失败只写终端投影，不挤占 Composer；原因、下一步和可展开技术详情位于终端输出滚动区。有效只读权限预先禁用创建；权限未知但当前连接已收到明确只读拒绝时同样阻止重复创建，stale 权限不作预检依据。权限可用时提供审批设置入口，未知时提供连接诊断；均不更改安全默认。运行中 I/O 失败保留终端标识与可操作状态，后续成功回执清除局部错误。

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
- 仓库入口 `scripts/pawork-desktop.sh` 支持 `build|start`：只构建正式 `pawork`/`pawork-desktop`，默认独立 `desktop` 实例，审批与逐项目信任跟随 Global 配置；显式环境参数仅覆盖当次启动；不加载 fixture、seed 或测试 profile。macOS 通过最小 `.app` bundle 执行真实二进制以获得正常窗口/AX 注册。

### 3.2 三栏工作台（100%：侧栏 288 / Inspector 440 / 状态栏高 30；窗口宽 ≤1279 时侧栏 240，Inspector 展开需 ≥1288；150%：侧栏 320、宽度不足 1320 时 Inspector 折叠；Workspace ≥560）

- **TaskRail（左侧栏）**
  - 顶部三行：标题行「Pawork」（20px semibold）+ ghost grouping 直接切换按钮 36×36（OPT-4a 命中区，字形 20px）；当前 Timeline 时显示 Projects glyph / `Show projects`，当前 Projects 时显示 Timeline glyph / `Show timeline`。按钮无 chevron、menu、expanded 或 selected child；AX value 分别为 `Timeline view` / `Projects view`。全宽 raised scope 行（h36 / 1px 描边 / r6 / 16px，All projects / 各 workspace）仍是独立菜单；连接行 Ø10 状态点 + 12px 状态文案 + 36×36 全局「+」（OPT-4a）。Reconnect 仅在 Disconnected / ConnectFailed 出现。
  - Scope 触发器显示「筛选 · 项目名」，仅过滤侧栏列表；若隐藏当前任务，Composer 下方明确说明任务归属未变。菜单末项固定 `Add project…`。系统目录选择器取消不改变项目 / 会话 / 草稿，成功后必须等 Host `workspace_add` 回执与 snapshot，再选择返回的 canonical workspace。
  - Timeline 分组 = 日期桶（Today / Yesterday / Previous 7 days / Earlier）→ 项目 → 任务；Projects 分组按 canonical workspace，缺 `workspace_id` 进 Unassigned（无「+」）。
  - UI-2 日期桶头 12px medium secondary；项目头可点折叠（14px，chevron ▾/▸ + 名称 + 右对齐任务计数共用完整悬停 / 焦点面；计数区同样可点击折叠）；项目级 36×36「+」（OPT-4a）按该 workspace 新建任务。
  - 任务行（UI-2：44px，选中 raised + 圆角 6）：状态点 Ø10（Needs input=琥珀实心 = 有待审批；Running=accent 蓝实心 = 该 session 有 active run；Blocked=danger 红实心 = R3 Wave B live 派生的 failed / interrupted 终态，优先级 Needs input > Running > Blocked；其余空心灰圆不声明语义；wire 无每会话终态字段故不画终态绿点）、标题单行 `.truncate()`（unread 时 SEMIBOLD，同字号同行高不改几何，不加 dot / 徽标）、14px 标题、8px 间隔、12px 相对时间右对齐（now / Nm / Nh / Nd）；时间与改名/归档共用 64px 尾槽，悬停、行或动作聚焦、当前会话时露出动作（标题宽度不变），归档为 16px 单色盒形图标（ADR-054，命中区 32×32；改名为行内编辑，Enter 提交 / Esc 取消 / 空白不提交；归档立即生效、仅隐藏不删除）；任务 click、行级 Enter、AX press、Cmd+Opt+↑/↓ 与 Cmd+Opt+N 都先关闭旧菜单并在切换后聚焦 Composer。激活当前 task 不重开 session 但仍关闭菜单；仅一个可见 task 时 cycling 不重开 session，仍聚焦 Composer。
  - grouping / scope 切换不改 active session 也不动分组展开状态，下一次 render 把 active task 滚动到可见（项目折叠时退回头部行；active 被 scope 过滤则诚实跳过）。
  - P0-2 grouping 直接切换只改本地 presentation：不改 active session、scope、Composer draft、Run 或 collapsed projects；切换时关闭其它浮层、清 menu highlight、保持按钮焦点，并置 `rail_scroll_to_active` 供下一帧找回 active task。
  - 底部账户区（F-04，TR-12 honest-hidden）：只保留「Local」本机身份行；头像 / 姓名 / quota / 组织等无权威来源元素一律不画。
- **Timeline（中栏上）**：虚拟化列表渲染五类条目——`You:` 用户消息、`Pawork:` 助手消息（流式增量合并为一条）、连续工具 group（标题汇总数量 / 状态，可折叠）、运行状态 / 唯一 terminal summary、`Error:` 错误行。空态（无 active session 且条目数为 0）居中显示 `Start a task`、一句 `Choose a task from the sidebar or create a new one.` 与唯一 Primary `New task`；此时 Header 不重复显示新建按钮。按钮与 Header 路径共用 `header-new-task` focus / handler，AX 同步 title / description / enabled action；Disconnected 保留旧条目时不显示空态。每条右侧「···」菜单含 Fork（仅 reducer 判定的闭合 run 边界可用；不可用时灰字禁用行；接受后聚焦 Composer）。用户上滚脱钩后右下浮出 `↓ Back to bottom`。
- **审批卡**：`pending_approval` 存在时作为 timeline 末项渲染——警示底色卡片（`Approval · {tool}` / reason / 可选 preview detail）+ 三按钮 Allow once（Cmd+1 / Cmd+Return，Primary）、Allow for run（Cmd+2，Success）、Deny（Cmd+3，Danger）；断线时禁用且 tooltip 给出原因。显式决策由 mouse / keyboard / AX 统一复核 gate，发出后关闭旧菜单并把焦点交回 Composer，避免卡片卸载后悬挂焦点。
- **Composer（中栏下，R5 Wave A / F-09）**
  - 单一 raised surface 使用 1px subtle border / r8；常态总高 88–94px（`COMPOSER_PANEL_MIN_HEIGHT=88`，不是输入框 min），增长上限 220px。两行：行 1 TextInput 单行约 28px（含 inset），多行向上增长；行 2 footer `items_center`，控件高 28px；Send/Cancel 同槽 36×36（OPT-4a）。
  - 卡片内动作行：model Dropdown 触发器（仅 `display_name`，provider / raw id 在 tooltip、菜单和 AX value；max_w≈220 truncate；run 进行中 / 目录未加载 / 断线禁用并 tooltip 给原因）与右侧发送 / 取消同槽。
  - UX-05 模型菜单：按名称 / ID 不区分大小写筛选，打开清空搜索并将当前项滚入视口；顶部固定搜索 / 清除和当前高亮模型的连接 / 来源，列表按供应商分组标明状态。来源仅取已有 `provider_auth_status`：远程 / 备用 / 不可用，备用目录明确不代表认证成功，未知或 stale 状态明确未确认；不硬编码隐藏模型。完整名称与 ID 可读，列表独立纵向滚动；鼠标悬停和 ↑/↓ 共用高亮，Enter 选择，Esc 返回入口。未搜索 / 未移动时 Enter 保持当前项以挡住 AppKit 重复开菜单按键；键盘选择后焦点落工作台根容器，避免同键重开或发送草稿。AX 取菜单和列表实际 bounds，不再猜测首帧顶部几何。

  - 卡片下方元信息：只读项目状态 → 无项目任务的「文件工具不可用」→「在项目中新建任务…」入口，右侧为 ContextMeter（`Context · — / {window}` 或 `unavailable`，不画进度条）。普通宽度同排，窄窗 / 大字号必要时换行；元信息高至少 28px，仍随字号增长。卡片与各元信息控件经 ScrollHandle 测量，AX 在 prepaint 后同步实际框。status_hint 和当前任务被筛选隐藏的说明分别显示；项目限制不另占行，有项目任务不保留限制与入口的空位。
  - 动作槽单按钮：视觉 element id 统一 `composer-action`，单一 `composer_action_focus`。idle/disconnected/no-session 显示 Send（32×32 圆形 Primary，↑；可用 tooltip「Send message (Enter)」；空/纯空白、无 session、断线、running 均 disabled + tooltip 给原因）；running 显示 Cancel（同槽 32×32 Danger，✕，tooltip「Cancel run (Cmd+.)」）。Send 点击与 AX press 均先判 `is_composing()`，组合中不发送。AX 节点 id 仍为 send/cancel 随态互换。状态切换两按钮同槽互换，面板几何与锚点零位移。
  - per-session 草稿：`HashMap<session_id, String>` + 无 session 独立槽；`open_session` 切换前 stash 当前 Composer 文本、切换后 `reset_text` 恢复（无则空，清 undo）；`MessageSent` 成功清该 session 草稿（可见 Composer 仅在回执属于 active session 时清空）；断线不动草稿；终端 TextInput 不参与。发送清空走 `clear()` 入 undo 栈，发送后 Undo 可恢复上一条文本。超长文本由父容器 max_h + overflow_y_scroll 承载，caret 滚进视口，面板总高仍受 88–94 / 220 合同约束。
  - 提示行删除。空输入 placeholder 只走状态机（不被 `status_hint` 覆盖）：idle=`Message Pawork… (Enter to send, Shift+Enter for newline)`；running=`Run in progress — sending is disabled. Cancel remains available.`；无 session=`Open a session to send messages.`；connecting/disconnected/failed 沿用既有文案。瞬态反馈（Forked / 发送失败等）落 footer Label，发送失败在输入非空时也可见。非空输入时状态原因仍由 tooltip + AX 承载。
  - 诚实缺省：不画 reasoning、附件/纸夹、follow-up/queue；ContextMeter 维持文本；workspace 归属只读。UX-03 的新建入口在空态和无项目任务中提供项目列表及添加目录，选择已有项目立即创建并打开新任务；添加目录沿用 `workspace_add`，内部 `WorkspaceOpened.create_task` 随当次请求携带创建意图，收到 Host snapshot / canonical id 后才调用既有 `create_session`。不重绑旧会话，草稿沿用原有逐会话保存；目录取消不派出 workspace / session 命令。菜单鼠标、↑/↓、Enter、AX 共享选项，Esc 返回入口焦点。
  - ADR-054：All projects 范围下全局 New task 直接创建无项目会话（`session_create` 不带 workspace_id），归 Unassigned；项目作用域与项目头「+」仍定向绑定新建。
- **Inspector（右栏，cmd-i 开合）**：顶层三页签，默认 Changes；UI-1 顶层条 48px / 每项 100px / 14px 字，Changes 内二级条 40px / 每项 96px / 12px 字，选中态为轻背景与居中 24×2px 中性下划线；各页滚动状态独立保留。
  - Changes：Files / Summary 二级页签 + ↻ 手动刷新。Files = 文件清单（路径 · status · `+A/−D`，≤200px 内滚动）+ DiffView（等宽 Menlo；hunk 头 raised 底 secondary 字；addition 行 success_bg 底 / deletion 行 danger_bg 底 / context 行 panel 底；长行 `overflow_x_scroll` 横滚不折行；binary 显示「Binary file — not rendered.」）。Summary = 七字段行（Session / Files / Lines / By status / Branch / Dirty files / Work dir，缺失显 unknown）。数据会话 ≠ 查看会话时顶端 banner 如实标注。
  - Terminal：host 流式 `TerminalOutput` 滚动文本（非 VT100、无本地 PTY）+ cwd 与尺寸组（`−W +W [列×行] −H +H`：stepper 只改本地草稿并钳制在 20–500 列 / 6–200 行，可见值与 AX value 同源；尺寸按钮把草稿经 `terminal_resize` 下发，在途时禁重复提交，仅匹配当前终端与当前草稿的回执才清草稿，终端切换也复位；缺/空 cwd 事实显示 `unknown`，G3）+ 终端输入（Enter 写入，未启动时先懒创建；write/resize 瞬态失败在终端仍 running 时不锁死，仅 status_hint 报错，G2）+ Start / New / Size 单槽（未创建 Start；可操作 Size；已知 exited/killed 变 New——同 workspace/cwd 新建终端，旧终端只读保留；failed 须先 Close 清理后回到 Start；create/resize 在途时同 gate 禁用，G2）+ ADR-045 Stop/Close 同槽（running Stop，终态 Close）+ 脱钩回底控件。
  - Resources：MCP server 只读表（name + state 徽标，`failed` 红字；`transport · N tools[ · last_error]` 次行）+ ↻ 刷新。
- **StatusBar + ActivityPopover（R6 Wave A / P1-4）**：UX-07 底部 30px StatusBar 显示当前 / 最近 Run 的输入、输出 tokens（缺失为 `本轮用量 —`），运行中附加时长。OPT-4b（F6）起 Inspector 默认折叠（宽屏同样），显式动作（重开 / Activity 摘要 / Review changes）才展开。Inspector 展开态由面板内右上 `inspector-collapse`（36×36、20px 字形）折叠；折叠态 Workspace Header 右上 `inspector-toggle` Activity 触发器与最右 `inspector-expand` 重开按钮并存（各 40×37 槽、4px 间距），点击后在触发器下方以右缘对齐展开 Popover（内容宽 320px，内容高随字号为 144/180/216px）：标题 Activity、Changes 标题与权威摘要 `N file(s) · +A/−D` 或 `unavailable`；点击摘要或 Run Summary 的 Review changes 均展开 Inspector、定位 Changes 并聚焦当前选中的 Changes 顶层页签；仅有 latest 会话差异时加来源说明。Agent/Add tool 无 capability 不画，也不为其保留空白高度。

### 3.3 键盘与焦点

按钮、列表行、Switch、Settings 导航与 Inspector / Changes 页签统一使用按输入方式显示的焦点提示：鼠标点击只有 hover / pressed 与业务选中态，不显示焦点边框；Tab / 方向键导航显示 2px 中性描边，后续鼠标按下立即清除。选中背景、页签下划线和 Switch 位置由业务状态决定，失焦不改变；焦点链、Enter / Space 激活与 AX 语义不变。

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
| AppView | cmd-= / cmd-+ | IncreaseTextSize——100% → 125% → 150%，顶档 no-op |
| AppView | cmd-- | DecreaseTextSize——150% → 125% → 100%，底档 no-op |
| AppView | cmd-0 | ResetTextSize——回到 100% |
| 浮层菜单（Scope / Model / Entry / Activity 打开时，不要求触发器聚焦） | up / down | 移动键盘高亮（wrap；未移动时从当前选中项起算，菜单关闭复位；单行菜单为 no-op） |
| 浮层菜单（同上） | enter | 选择高亮行（等价点击对应 MenuRow；触发器 keyup 合成 click 由衔接标记吞掉防重开） |
| TaskRail 列表 | up / down | 焦点沿 §3.6 停靠链步进（clamp 到两端，不 wrap；带修饰键不接管） |
| TaskRail 列表 | enter / space | 激活聚焦行：ListRow 行级 key_down 直接调用与 click 同一激活 handler（打开 task / 展开收起项目）；Scope/Model 等菜单打开时行级让位，Enter 由根节点菜单接管 |
| TaskRail 项目头聚焦时 | left / right | 收起 / 展开（已处目标态 no-op） |
| 根节点 | tab / shift-tab | focus_next / focus_prev——沿 tab_index 档位链走焦（GPUI 无默认 tab cycle；macOS 上 NSWindow 在 sendEvent 层吞掉裸 Tab，keyDown 路径收不到，主机制是 `install_appkit_tab_monitor` 的 NSEvent 本地监听器在派发前截获并调 `window.focus_next()` / `focus_prev()`，根节点 on_key_down 的 Tab 分支保留作非 macOS / 监听器失效后备；带 cmd/ctrl/alt 的组合键放行不接管） |
| 根节点 | escape | 浮层菜单（任意 `MenuKind`）打开时关闭并把焦点送回触发器（不要求触发器聚焦）；其余情况关闭当前浮层菜单 |

主路径按钮（`MAIN_PATH_TAB_STOP_IDS` 九项：approve-once / approve-for-run / approve-deny / composer-action / add-task / header-new-task / reconnect / model-picker / timeline-back-to-bottom）挂 tab_stop + track_focus + 聚焦描边三件套；Timeline 行级动作（Review changes `run-review-<event_id>`、Entry 菜单 `entry-menu-<event_id>`）按 event_id 懒建行级 FocusHandle，不进静态清单。Composer 动作槽视觉 element id 为 `composer-action` 单槽；AX identifier idle=`send` / running=`cancel` 随态互换。

rail 聚焦 Button 上裸 Enter / Space 为行级键盘激活：grouping 直接调用 `toggle_grouping` 并关闭其它浮层；scope / add-task / reconnect / 项目定向「+」仍走各自 click 同源路径。激活后的同键 keyup 合成 click 由 `pending_button_key_activate` 吞掉，防双触发；disabled 不激活。

Tab 焦点顺序（design §3.6，R3 Wave B）：rail 前缀三档 `RAIL_TAB_STOP_IDS`（-20/-19/-18：project-scope → task-rail-grouping → add-task）→ reconnect -17 档（R7 Wave A 接入；仅断线态渲染，不渲染时自动退出 Tab 链）→ rail 行级 -16 档（项目头 / 定向 ProjectAddTaskButton / task 行，按当前分组渲染序；折叠项目只保留头部）→ 主路径 `MAIN_PATH_TAB_STOP_IDS` 0 档 → composer `COMPOSER_TAB_INDEX` 1 档链尾（wrap 回 rail 首停）。Tab / Shift-Tab 经 AppKit 本地监听器（后备：根节点 on_key_down）映射 `window.focus_next()` / `focus_prev()` 真实可走（Slice 4）；菜单键盘导航在浮层菜单打开时即接管（Slice 5 起不再要求触发器聚焦；rail 与行级 / 按钮级激活在菜单打开时让位，冒泡到根节点裁决），Escape 关闭一律回焦触发器。

AX 焦点口径：grouping 是直接按钮，name 表达目标动作、value 表达当前视图且 Press 后焦点仍在按钮；其余浮层菜单打开时触发器让出 focused，高亮菜单项成为树内唯一 focused 节点。Timeline 行级动作仍与 click 同 handler / gate。

UI-6b 命名账号（[ADR-059](../settings.md#adr-059ui-6b-命名账号与持久选择2026-09-08)）：Settings credentials 显示名称、kind、掩码、过期与后续请求选择；名称输入与 API key/OAuth 新增复用 inline 流程。行与 Use/Remove/确认按 credential ID 绑定，AX 与键盘动作同源，选中账号仍有其他记录时禁删。握手 minor ≥15 才显示新增账号能力；旧 Host 保留原操作与旧状态解码。controller 通过现有 pawork-client 通用命令发送，不新增业务依赖。

UX-06（2026-09-09）：供应商列表按已连接优先稳定排序，render / AX 共用顺序，不改 Host 目录与认证语义；四默认角色移到列表后，识图 / 搜索的选择入口与说明直接标注「尚未生效」，仍允许保存偏好。展开区先展示账号 / 当前选择 / 三窗额度 / 账号操作，再显示添加账号、代理与模型管理；不同账号以间距与分隔线区分。当前选择只作用于该供应商后续请求，进行中请求保持原账号。窗口已用 / 剩余分别读 canonical 字段，剩余缺失显示未知，不按已用推算；倒计时保留分钟精度并分解为天 / 小时 / 分钟，说明向上取整与不确定时间，已到重置时要求刷新确认。各窗保留来源与获取距今秒数，旧缓存 / 超 30 秒 / 未来时间仍明确标过期。订阅三窗和多个 Key 不相加；底栏「任务额度 —」没有任务额度读数，不复制 Settings 的账号额度。无 wire / schema / 依赖变化。

### 3.4 语言（i18n）

- 界面 chrome 文案集中在 `ui/i18n.rs` 目录，`t(key)` / `t2(key, a, b)` 按当前语言返回 `&'static str`；未知 key 原样返回，不 panic。
- 语言切换入口在 Settings → Appearance（`English` / `中文` 两个同源按钮，AX identifier `settings-language-en` / `settings-language-zh`），mouse / Enter / Space / AX Press 共用 `AppView::set_language`：更新全局语言与本地镜像、刷新 terminal_input placeholder、给出 status_hint 并整界面重渲染。
- ADR-053：保存到用户配置目录 `desktop.json`，重启恢复；缺文件默认 English。不触碰 Host / 会话 / Run 状态。
- 翻译边界：只翻译 chrome 文案（按钮、提示、空态、状态提示、tooltip、Settings 页）；session 标题、provider / model id、文件路径、工具输出、wire 错误原因等数据内容保持原文；品牌名「Pawork」、功能符号（✕ ↑ ⚙ + ▤ ◷ ↻ ↓）与示例数据不翻译。
- render 与 AX 经同一 `t()` 调用同源取词；AX 节点 id 保持英文。

#### UX-07 工具与运行事实（2026-09-09）

展开工具共用 render / AX / 测高的参数、结果文本，参数 JSON 格式化显示；明确区分“尚无结果数据”“结果为空”，有权威目录 metadata 时显示相对路径与“0 项（空目录）”。工具完成及 Run 终态通知后，controller `refresh_timeline` 使用现有 SessionGet 从当前 Run 已知最早序号前补读，reducer 合并历史，不清草稿和时间线；跨任务迟到页继续经 session gate 拒绝。持久化用户消息回填时替换同 Run 的本地回显；迟到的发送回执不再追加第二份。

Run 页脚和底栏使用 `run_usage_label` 显示对应 Run 输入 / 输出 tokens。累计值仅取持久化 Completed / Failed / Cancelled 的 `usage`；零值有效，缺值未知，活动 Run 不沿用上一轮读数。取消只保留轻量页脚；失败卡保留原因，页脚不重复状态；Review changes 仅挂到当前任务最近 Run，不能把当前差异挂到早先回合。底栏移除无信息的常驻 quota / tok/s / idle 占位，运行中保留真实开始时间驱动的时长；ContextMeter 与订阅额度范围不变。

复用现有投影、本地回显、工具文本与 AX 行高测试，并与 protocol 的实时/历史 golden 同批验证。无新增生产依赖、wire 类型或安全默认变化；真窗口与用户验收状态见 [路线图 UX-07](../../ROADMAP.md#ux-07-工具与运行事实可核查)。

## 4. 核心行为与数据流

UI-6b G2：新增 `ui/settings/provider_quota.rs`；Controller 通过 Client 查询指定 provider/credential 的 Percent 三窗与写选择模式。投影按稳定 ID 和请求 epoch 存读数，结合 connection generation 拒绝迟到响应；离页/断线/认证变化清理。账号区显示已用 / 剩余百分比、重置时间、来源、获取时间、刷新与过期标注，既有每秒时钟在 30 秒或 reset 后更新状态；仅新 Host/Go 开放自动切换开关。按钮/开关及 AX 共用实际布局，扩展既有 Provider AX 回归。

### 4.1 启动 → 连接 → snapshot → 分页 timeline → live 事件 → 断线 Reconnect

1. `AppView::new` 即 `start_connect`；`DesktopController::connect` 先按 socket 文件名推导 token 路径（`pawork-gui-X.sock` → `gui-X.token`）并读 `gui.token`——缺失 / 不可读 / 为空即整个连接失败（fail-closed）。
2. 建 512 容量 `smol::channel`；`LocalTransport` + `ConnectOptions{ timeout 10s, client_label "pawork-desktop", 帧上限 1MiB }`；带上内存中的 last_acked（若有）。
3. **在 `runtime.spawn` 内**执行 `GuiClient::connect_with_resume_config`（含握手与能力宣告）→ 取 `initial_snapshot` → 按 resume 三态 ack：首连记录并 ack `snapshot_sequence`；`Replay` 记录并 ack `through_sequence`；`UpToDate` 只记录 `current_sequence`；`SnapshotRequired` 换用 `outcome.snapshot`（无则回退握手首帧）再记录并 ack → `subscribe_all`。连接期任何 client 调用都不得落在 gpui 前台执行器上（见 §8 崩溃教训）。
4. UI 侧 `on_connected`：无 resume 走 `apply_fresh_snapshot`（原 active session 仍存在则重新打开）；有 resume 走 `apply_resume_outcome`——`Replay` 由 reducer 按 sequence 续接重放事件、不闪全量重载；`SnapshotRequired` 先丢 stale 权威标记再换基线并重分页；`UpToDate` 保留 Timeline，但合并握手 snapshot 中无法由事件恢复的权威状态（尤其 terminal 终态）。任何成功重连都刷新当前打开的 Changes/Resources；断线不清 `active_session_id` / unread / blocked。
5. 打开会话（`open_session`）：先关闭任何旧 `MenuKind`，再由 `select_session` 无条件清 timeline / seen / tombstone / tool anchors 并恢复 snapshot 中该会话的 active run 与 pending approval → controller 按 `session_get{ timeline_after_sequence, timeline_limit: 500 }` 链式分页（至多 200 页）直到 `complete`，每页发 `TimelineLoaded`；分页期间先到的 live 事件由 reducer 按 sequence 去重。用户发起的 task click / 行级 Enter / AX press / cycling / next-needs-attention 调用方在切换后统一聚焦 Composer。
6. 事件泵：`next_event_timeout(1s)` 循环——收到事件即记 last_acked（单调 max）、回 ack、投 `ControllerEvent::Event`；保活为独立 tokio interval 任务，每 15s 发一次 `heartbeat()`，不随事件泵 / UI 排水阻塞（host `heartbeat_timeout` 30s，任意入站帧刷新；client io 为 AsyncMutex，支持泵内并发调用）。
7. 心跳失败或泵错误：对照连接 generation，仅首次清空本代次 client 槽时发 `Disconnected{reason}`（泵与心跳同时失败不连发；代次已变则静默退出）。UI 置连接态并提示「Connection lost. Click Reconnect.」。用户点 Reconnect 重走 `start_connect`——带 last_acked 走 resume，不永远全新 Snapshot。

### 4.2 发送一次消息到流式渲染

1. Composer Enter（先判 `is_composing()`，IME 组合中的 Enter 属输入法确认直接返回）或 Send 点击 / AX Send press（同样先判 composing）→ `can_send`（Connected + 有 active session + 无进行中 run + 文本 trim 非空）才可点；空/纯空白 Send disabled，tooltip「Message is empty.」。
2. `run_start{ session_id, user_message[, provider, model] }`（模型取 `effective_model` = pending 优先于 selected，只影响下一轮）→ `Accepted{run_id}` → `MessageSent`（回执携带发送文本）调 `note_session_run`（乐观写入 `active_runs`，active session 同时设 `active_run_id`）与 `note_user_echo`（本地乐观回显：active session 立即上屏 UserMessage 行并 bump 时间线代次）并清空输入框，不等 live `RunChanged`；wire 无用户消息事件（`MessageCommitted` 不进实时流），重选 / 重连后由快照重放的持久化行替换回显行。
3. live 事件流（`RunChanged` / `AssistantDelta` / `ToolStarted` / `ToolOutput` / `ToolCompleted` / `Diagnostic`）经 `projection.apply_event`：时间线语义（sequence 去重、有序插入、assistant 按 message_id 增量合并、committed 替换 tombstone、tool 双键锚点回填）全在共享 reducer；本包只更新 UI 态（run 跟踪、审批卡、blocked 派生、非 active session 的 unread 标记、`model.switched` Diagnostic 确认模型切换）。共享 reducer 在 live / history 两臂均只展示 `sandbox.fallback` 运行提示，`resources.injected` 等信息诊断不会因历史重放变成 Error 行。
4. 时间线每次变化 `timeline_changed()` 递增代次；render 前 `sync_list` 对 `ListState` 统一 `reset(len + pending_approval)`（projection 有条目替换语义，splice 不安全）。R4 Wave A 起为 Top 对齐 + 显式跟随：跟随态由 `timeline_following` 单一表达（滚动事件 `visible_range` 覆盖末项即贴底），reset 后跟随臂显式 `scroll_to` 末项底；脱钩读史恢复 reset 前偏移（item_ix 越界钳制），视口不跳；回底 = BackToBottom / 滚回底部重挂。
5. run 终态（completed / cancelled / failed / interrupted）清 `active_run_id`（Composer 恢复可用）、清该 run 的审批卡，并触发 Changes 刷新；run 进行中由 1s 时钟驱动时长徽标重绘。

OPT-2 审查修复（2026-09-06）：`session_create` 仅从成功 Data 回执的非空 `session_id` 定位新会话，拒绝 Error / 缺键 / 畸形回执，snapshot 只刷新列表。当前会话因归档从 snapshot 消失时，AppView 保存原会话 Composer 草稿、恢复无会话草稿、复位分页与 Changes、收口 Terminal workspace 与对应输入草稿；重复 snapshot 仍按 UI scope 选终端。定向测试：`created_session_id_uses_receipt_and_rejects_invalid_responses`、`archived_active_session_restores_drafts_and_terminal_scope`（真实 AppView handler 连续两次刷新）。

### 4.3 审批卡交互

1. live `ToolApprovalRequired{ run_id, tool_call_id, reason }` 或 snapshot `pending_tool_approvals` 段 → `pending_approval`（tool_name 从 reason 首段提取；snapshot 形态含 relative_path / preview）。
2. 按钮点击、AX press 或 Cmd+1 / Cmd+2 / Cmd+3 都回到同一 `on_approve` 并再次复核 `can_approve` → `tool_approve{ run_id, tool_call_id, decision }`，decision ∈ `approve_once` / `approve_for_run` / `deny`；发出后关闭旧菜单并聚焦 Composer，避免审批卡卸载后焦点悬挂。
3. 清卡路径分 live / history：live `ToolCompleted` 按 `run_id + tool_call_id` 精确清除，live 或历史 run 终态按 `run_id` 清除；分页重放的历史 `ToolCompleted` / `ApprovalResponded` 不改写 pending（P4 片 2F 修 D3：同 run 可串行多个工具，更早工具的历史完成/响应不能抹掉 snapshot 中更晚工具的当前审批；历史 `ApprovalResponded` 不含 `tool_call_id`，无法安全定位，恢复只认 snapshot `pending_tool_approvals`）。无任何默认放行：不操作则永远 pending，断线时按钮禁用。

### 4.4 菜单开合与键盘激活语义

- 四种浮层（`MenuKind`：Scope / Model / Entry(event_id) / Activity；WorkspaceConfirm 已随 OPT-2a 移除）共用单一 `Option<MenuKind>` 状态位：开新即关旧、至多一个打开；Grouping 不再是 MenuKind。
- 行级键盘激活：聚焦的 ListRow / Button 上裸 Enter / Space 调用与 mouse click 同一 handler。grouping 的三种可见输入与 AX Press 都收敛到 `toggle_grouping`；其它菜单打开时行级 / 按钮级让位给根节点。
- Scope / Model 等菜单打开即接管，根节点承接 ↑/↓ 高亮、Enter 选择与 Escape 回焦；`pending_keyboard_menu_select` 防选择后的 keyup 合成 click 重开菜单，任何关闭路径复位高亮。
- 关闭路径：选择选项 / 再点触发器 / Escape（根节点 `on_key_down` 冒泡承接；面板经 `deferred` 绘制不可聚焦，组件层不可达）/ 外点（`MenuPanel::dismiss_on_outside` 的 `on_mouse_down_out`）。
- 外点关闭先于触发器 click 到达时，以 `(MenuKind, 按下位置)` 衔接标记判定「同一次物理点击」——位置精确相等才视为关闭收尾不重开；键盘触发无位置永不误判。
- 面板 `occlude()` 拦截下层点击与滚轮（无穿透）；超高在 240px 内自滚。
- Model menu 按 provider 首现顺序分组，组内保持目录顺序；可见行、↑/↓/Enter 与 AX 共享分组后的扁平索引。Composer 邻近底边，菜单以 8px 间距优先向上打开，`anchored` 仍在不足空间时贴合窗口边界；长 label 截断。
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
| 连接 / 重连 | 握手 + resume + subscribe_all | `DesktopConnect{snapshot, resume, handshake, events}`；handshake 仅保留非 Secret runtime/API/capabilities 摘要 |
| 添加项目 | `workspace_add` → 重取 snapshot | `Snapshot` + `WorkspaceOpened{id,name}` |
| 打开会话 | `session_get`（分页查询） | `TimelineLoaded`（逐页） |
| 新建任务 | `session_create`（workspace_id 可缺省）→ 重取 snapshot | `Snapshot` + `SessionCreated` |
| 会话改名 / 归档 | `session_rename` / `session_archive` → 重取 snapshot；Host 写后另广播 `SessionMetaChanged` 再刷新 | `Snapshot` |
| 发送消息 | `run_start` | `MessageSent{run_id}` |
| 取消 run | `run_cancel` | 经事件流 `RunChanged` 收敛 |
| 审批决策 | `tool_approve` | 经事件流收敛 |
| Fork | `session_fork` → 重取 snapshot | `Snapshot` + `SessionForked` |
| 终端 | `terminal_create` / `terminal_write` / `terminal_resize` | `TerminalCreated` / 流式 `TerminalOutput` |
| 模型目录 | `model_list` | `ModelsLoaded` |
| Settings 供应商状态 | `provider_auth_status` | `ProviderStatusLoaded`（含顶层 default） |
| 设为默认模型 | `set_default_model` → 重查 `provider_auth_status` | `DefaultModelConfirmed` + `ProviderStatusLoaded` |
| Settings 通用设置 | `general_settings` | `GeneralSettingsLoaded` |
| 设置 / 清除代理 | `set_proxy_url`（null=清除） | `ProxyUrlConfirmed`（回执即写后状态） |
| Provider 代理开关 | `set_provider_use_proxy`（ADR-052） | `ProviderUseProxyConfirmed`（回执即写后状态） |
| Settings 权限设置 | `permissions_settings` | `PermissionsSettingsLoaded`（四元组，含 Host attached workspace_id） |
| Settings 工具与 MCP | `mcp_list`（复用）+ `mcp_test` / `mcp_server_remove` 命令 | `McpServersLoaded`（复用 ResourcesPanelState）/ `McpServersReceipt`（写回执即权威） |
| 切换审批模式 / 会话信任 | `set_approval_mode` / `workspace_trust` | `ApprovalModeConfirmed` / `WorkspaceTrustConfirmed`（回执即写后状态） |
| Changes | `diff_list_files` / `diff_get` | `DiffFilesLoaded` / `DiffContentLoaded`（带 epoch） |
| Resources | `mcp_list` | `McpServersLoaded`（带 epoch） |
| 任意失败 | — | `OperationFailed{action, reason}` |

domain id 类型未从 client re-export，命令 / 查询经冻结的 serde 形状（`method` / `params` JSON）构造，避免引入第二个业务依赖；`CommandSource::Automation` + `ActorIdentity::System` 仅为信封占位，服务端 host_stamp 统一覆盖为 LocalGui + LocalUser。

## 5. 契约与不变量

- **UX-02（2026-09-09）**：常用管线表格按等宽列渲染，支持表头、对齐、中文、escaped pipe / 代码内 pipe；未完成或不匹配行保留正文。`message_body_element(entry_id, text, color)` 用事件 ID 隔离块内控件，`message_actions` 提供代码原文复制与编号 HTTP(S) 链接打开 / 复制（保留配对括号）；标题 / escaped URL 等完整 CommonMark 语法不在本子集内。正文菜单复制消息原始 Markdown，不拼作者 / 时间。`timeline_entry::fork_target` 仅将助手回复映射到相同非空 `run_id` 的后继合法闭合边界，原边界直接使用；连接 / active session gate 继续复核。鼠标、菜单键盘与 AX 共用动作序列；`entry_menu_scroll` 支持高亮滚入，AX 从实际布局裁剪可见项，布局后同步避免首帧缺节点，禁用分叉有可见说明。无 schema / wire / 依赖变化。已实现、221 项自动检查与 build 通过；历史回复真窗口、复制 / 分叉与三档字号通过，新请求流式验收因 HTTP 401 待补，用户未验收，详见 [路线图](../../ROADMAP.md)。
- **UI-3（2026-09-08，进行中）**：Markdown 正文与工具组默认折叠已实现。`expanded_timeline_details` 以首个 event id 保存显式展开状态，render / AX / 测高同源；输出完整换行，工具行不再限定总高度。旧 RunPhase 由同 Run 后继状态吸收，完成且无 Changes 时仅留页脚，失败/取消与 Review 卡保留。用户视觉反馈后改为 880px 居中列、16px 正文、26px 行高、12px 作者间距与段距、32px 消息间距（条目 padding 参与虚拟列表测高），用户消息为 20px 内边距浅底卡片；工具摘要 36px、无底色，展开保留细边线；详见 [UI-3 规格](../../gui-design.md#ui-3-时间线更新2026-09-08进行中)。ADR-057 补齐共享思考投影与 API 1.14，持久事件与业务依赖不变。思考为独立默认折叠的 36px 摘要，展开显示 14px secondary 全文、四边 12px 内边距；使用稳定 run/message 键与工具共用展开/焦点机制，测高、render、AX 同源，收起时 AX 不含正文。既有 AX 交互测试追加思考折叠/展开与视口保留断言；实际窗口/Provider 状态以 [路线图](../../review/roadmap-ui-2026-09-09.md) 为准。
- **UI-1（2026-09-07）**：共享中性深色 token、6/8/12px 圆角，Header 80px（顶留 24px，标题 18px，动作 40×37px），Inspector 页签 48/40px，状态栏 30px。InspectorMotion 只保存瞬时宽度，以 180ms cubic ease-out 开合，反向连续，空间不足直接收起；每次实际宽度变化（含终帧）刷新 Timeline 测高缓存；AX 使用本帧相同宽度并裁去不可见动作，不改变偏好 / 四层架构 / wire。hover、pressed、焦点即时反馈。新规格见 [GUI 设计](../../gui-design.md#ui-1-工作台视觉更新2026-09-07)，旧阶段尺寸与色值不覆盖本项；自动验证与用户人工验收分开记录，后者已于 2026-09-07 通过。

- **视觉基准事实源**：[../../../design/README.md](../../../design/README.md)（P0–P2 三张 1440×1024 逻辑尺寸阶段目标设计图） 与 [../../gui-design.md](../../gui-design.md)。P0-1 已把基础字阶、六档 spacing、三档 radius、2px focus ring、icon/menu 几何与 hover/pressed 状态冻结到 `theme.rs`；普通 panel 无 shadow，menu/popover 才有 elevation。2026-09-04 已完成三张阶段图的当前构建真窗口视觉签字；2026-09-05 又完成 P0 系统 IME 与 P1 真实 Provider / Review / Approval 补证，并完成 P2 八页、三档字号与窄窗收尾复验；实际记录见 [Desktop Spec §8](../desktop.md#8-gui-收尾验收记录2026-09-05)。以上证据仍不得由 token 测试替代，也不扩张为跨平台或发布级保证。
- **R9 / P1 可见层级合同**：Timeline 的早期 618px wrapper 与 40/12px 节奏已由 UI-3 居中列与当前间距覆盖；TaskRail 项目计数 56px meta 槽，UI-2 任务时间 / 动作共用 64px 尾槽；Composer surface 与 unavailable 对比；Changes 20/72/76px 文件槽、36px 横滚外 header、24px gutter；ActivityPopover 内容宽 320px，高度随字号为 144/180/216px，外框另计 padding/border，保持 capability honesty。`Ready for review`、Review CTA、summary 行高与 AX 都由 active session 的非空可审阅文件 gate 同源驱动；空文件列表必须显示轻量 `Run completed`。这里只冻结可见实现；阶段图与 P1 真窗口签字已单列通过，但不扩张为 Timeline/Changes 全状态 AX 几何的自动覆盖。
- **审批 fail-closed**：无默认允许；决策只能来自显式点击或快捷键；断线禁用；run / tool 终态与 `ApprovalResponded` 清卡防幽灵审批。
- **P1 Run 工作单元（2026-09-04，2026-09-05 收口）**：不改 reducer / wire / sequence，只在 `timeline_rows()` 的既有 run/order 上组织视觉。连续 tool 以首个 event id 为稳定 group key，标题汇总真实数量与状态并可折叠；terminal summary 吸收同 Run 的重复相位，完成、失败、取消不混写。只有当前 Session 的 Changes 为 Ready、非 stale 且至少包含一个文件时显示 `Ready for review` / `Review changes`，mouse / keyboard / AX 进入同一 Changes handler；空列表显示 `Run completed`。Approval 继续占最高层级并保持三决策 fail-closed。Inspector 三页共享诚实 empty/error/stale 语言，Activity 仅呈现 Changes，内容宽 320px，高度随字号为 144/180/216px。
- **`gui.token` fail-closed**：token 缺失、不可读或为空即连接失败，禁止无认证静默连接；错误信息只含路径，token 内容不落日志。
- **Enter / IME 语义**：keybinding 仅 `TextInput` 聚焦时生效；Enter 冒泡到 AppView 后结合 `is_composing()`（`marked_range` 存在即组合中）与发送可用性裁决；Shift+Enter 恒为换行；终端输入框同规则。
- **UI-4 Composer（2026-09-08）**：卡片与正文居中对齐，留白、输入 / 动作与外部项目 / 上下文分层见 [GUI 设计](../../gui-design.md#ui-4-输入栏更新2026-09-08)。AX 对齐卡片内模型 / 发送坐标，补充真实 ContextMeter 节点；无项目提示与瞬态反馈不覆盖操作区。自动检查与真窗口状态见 [路线图](../../review/roadmap-ui-2026-09-09.md#ui-4-本批证据2026-09-08)。
- **Composer 草稿与空输入**：per-session HashMap + 无 session 槽；切换 session 先 stash 再 restore（`reset_text`，终端不参与）；`MessageSent` 成功清该 session 草稿，断线保留。空/纯空白输入使 Send disabled（tooltip「Message is empty.」），消除空点击面。
- **OAuth 登录交互（2026-09-08）**：统一按 `AuthStartData` 呈现「打开授权链接」「复制链接」与可选「复制验证码」，三种输入路径共用 Settings action handler。打开只允许 HTTP(S)，需要当前 Connecting 且写 gate 有效；复制使用 Host 原始值，无额外标签。登录详情为可选中复制的只读 `TextInput`，长 URL 横滚、AX TextArea 只发布 Focus 不发布 SetValue；错误、端点和到期信息同样可复制。`read_only()` 保留选择 / Copy，屏蔽编辑、Undo / Redo 与 IME，内部 `reset_text` 更新仍有效。终态不再提供授权动作，离开 Settings 清理详情实体及复制反馈。未改 Host / wire / Secret 语义；行为定向验证与真窗口验收分别记录于 [Desktop Spec](../desktop.md)。
- **Settings 默认模型（SET-5）**：默认项只在 Host `set_default_model` Data 确认后更新 Composer（`selected_model` 同步、清 pending），随后重查 `provider_auth_status` 落地权威 `default`；失败走 OperationFailed 不落地乐观状态。默认失效（provider 未连接，或已加载目录不含该 model，包括成功加载为空）显式提示；尚未成功加载的空目录不判定不误报，不静默切换；`Set default` 要求 provider 已连接、非 stale、非当前默认（四路径同 gate）。刷新失败保留旧列表与默认项。
- **P2 Settings 产品化（2026-09-04，2026-09-05 Network 收口）**：Settings Rail、标题、section、field、feedback 与导航统一 English，内容最大宽 820px（OPT-4c 起取消上限，内容全宽、两侧 32px padding）；Advanced 断线可达且 Settings 不渲染 RunStatusBar。UI-6a Provider 普通层为自然增高卡片，分组发布名称 / 认证方式与连接态 / 目录模型数；masked credential 永不进入普通 render 或 AX summary，endpoint / catalog error 只在连接、等待或二次确认详情出现，API key 编辑器仅由 Connect / Replace 打开，默认模型独立 section。Approvals 使用整行 radio；Network / Terminal 使用 label-help-feedback；Appearance 有即时字号样例；Advanced / About 为 definition list。100%/125%/150% 与 1080×720 继续走现有共享 token / layout；UI-1 后 Inspector 开合使用 180ms ease-out；当前无系统 Reduce Motion 偏好读取。
- **Settings Tools & MCP 页（SET-6c / ADR-049）**：导航仅在 Host `mcp_list` 至少成功一次后显示。清单复用 mcp_list（name/transport/state/tools/last_error）；每行 Test 发 `mcp_test{name}`、Remove 两步确认后发 `mcp_server_remove{name}`，回执 `{servers:[...]}` 即权威生效值（bump epoch，不另重查）；Error 保留旧清单并在本页显示失败文案（render/AX 同源 `action_error`，不再只进工作台 status_hint）；未知/畸形 fail-closed；断线 stale 只读禁写（render/键盘/AX 同 gate）。生效边界文案：remove 同会话生效（盘/密/内存三处一致），进行中 Run 已快照工具不回溯撤销，重启后与盘一致。
- **Settings Network 页（SET-6a / ADR-047）**：可见页名从仅承载代理的 General 收敛为 Network；为保持兼容，Host query / command、内部 identifier 与投影类型仍沿用 `general_settings` / `set_proxy_url` / General 命名，不 bump wire。导航仅在 Host 查询至少成功解析一次后显示（失败/未知隐藏且不渲染写入口）。页面显示 Host 权威 `proxy_url`；GUI Save 与手工编辑共用 workspace 外标准用户配置目录中的 Global `config.toml`，workspace `.pawork/config.toml` 中代理值被忽略。null 文案 `Not set (uses system environment variables)`；回执 `{proxy_url}` 才改生效值，不另重查。断线 stale 保留最后只读结果并禁写（render / keyboard / AX 同 gate）。新 OAuth/验证/目录探测同会话生效；当前活跃供应商模型流量于切换供应商或重启 Host 后生效。proxy URL 非 Secret，输入明文；畸形载荷 fail-closed。
- **Settings Providers 供应商级代理开关（ADR-052 SET-6h）**：provider 概览行动作区追加代理 Switch，仅 Global `proxy_url` 已配置时可见；`use_proxy=true` 显示 On（走代理），`false` 显示 Off（直连），tooltip 区分当前生效态。click / Enter / Space / AX Press 共用 `on_settings_toggle_provider_use_proxy`，派出 `set_provider_use_proxy{provider_id, use_proxy}`；不乐观更新，`ProviderUseProxyData` 回执即写后状态，失败走 OperationFailed 不落地。写只落 Global 层 `config.toml`（workspace 层代理仍被剥离）；Host 装配按生效代理：Global `proxy_url` 统一生效，仅该 provider 显式 `use_proxy = false` 时绕过。新装配连接（目录探测、切换 provider / model）同会话即用新生效值，已装配连接于切换供应商或重启 Host 后生效。`providers[].use_proxy` 为必填生效值（未显式 `false` 即 `true`）。
- **禁动符号**（R8 冻结面，bin 内测试钉住内容）：`APP_VIEW_KEYBINDINGS`、`install_keybindings`、`MAIN_PATH_TAB_STOP_IDS`。
- **Settings Approvals 页（SET-6b / ADR-048）**：导航仅在 Host `permissions_settings` 至少成功解析一次后显示（失败/未知隐藏且不渲染写入口）。五档审批模式使用整行 radio；当前值 selected，只对 enabled 的非当前行发布 mouse / Enter / Space / AX Press，并发 `set_approval_mode`，等 Data 回执才改生效值（不乐观更新）。会话信任开关发 `workspace_trust`；`trust_workspaces_global` 只读行，null 文案 `Not set (workspaces are untrusted by default)`。未知 mode / 畸形载荷 fail-closed；断线 stale 保留最后只读结果并禁写。
- **Settings Terminal 页（SET-6d / ADR-050）**：导航仅在 Host `terminal_settings` 至少成功解析一次后显示（失败/未知隐藏且不渲染写入口）。页面显示 Host 权威生效值：shell null 文案 `Not set (uses the platform default)`，以及 columns/rows；Save 全态回传三字段，Clear 清除 shell，回执后才更新。新建终端初始投影尺寸与创建后 resize 取生效值（未查询回落 80×24）；只影响之后创建的终端。畸形载荷 fail-closed；断线 stale 保留最后只读结果并禁写，重连自动刷新。
- **依赖边界**：生产 `pawork-*` == `{pawork-client}`（deny-list 测试）；`projection/` 零 gpui / tokio / OS import；协议类型只经 client re-export。Settings Data 缺 nullable 键 fail-closed（与 Host 必填键对齐），不把残缺帧当成未设置。
- **时间线单一 reducer**：条目去重 / 合并 / 锚点 / resume 基线语义全部委托 `pawork_client::projection`（protocol 共享 reducer，`TimelineEntry` / `TimelineEntryKind` 直接 re-export）；本包只保留 UI 态与渲染分组。timeline 任何变化统一 `reset(count)`，禁 splice。
- **用户消息乐观回显（R4 Wave B）**：wire 对 `MessageCommitted` 返回 None（用户消息不进实时流），`MessageSent` 回执即经 `note_user_echo` 在 active session 直接 push 一条 UserMessage 行——event_id `local-echo-{run_id}`、timestamp 取 UI 注入的 now 毫秒串、sequence 借用当前最大 wire sequence（不进 `seen`、不占号段，后续 wire 事件严格更大自然落在其后）；非 active session（发送后已切走）不 echo，重放会补；重选 / 重连后 `select_session` / 快照重建 timeline 基线，回显行由持久化 evt- 行替换。禁止为此改 protocol 共享 reducer 或新增 wire 变体。早死路径（plan 闸门拒绝）的宿主合成 `RunChanged{Failed}` 携带 ≥2^60 合成序号（app 侧 `SYNTHETIC_SEQUENCE_BASE`），有序插入落在回显行之后而非时间线顶端（R4 Wave B 评审 P2 修复，投影级回归 `synthetic_terminal_after_user_echo_lands_at_bottom`）。
- **Timeline Top 对齐四合同（R4 Wave A，F-06）**：短会话从 Header 下开始不再沉底；跟随态由 `timeline_following` 单一表达，新内容只在用户贴底时追加跟随；脱钩检测走滚动事件事实——`visible_range` 覆盖末项即贴底、末项滚出即脱钩（Top 对齐下 is_scrolled 滚动过即恒真不可用；handler 内读 ListState 会在 gpui scroll() 写借用存活期重入 panic，且未测高项使像素 max 系统性低估，评审 P0/P1 修复），上滚脱钩不抢滚动、BackToBottom 重挂；条目变化仍统一 `reset(count)` 禁 splice，脱钩恢复钳制偏移。可读列 `TIMELINE_READABLE_WIDTH`=880 居中、两侧至少各留 28px，正文长行必须 wrap。
- **Workspace Header 诚实口径（R4 Wave A，F-05；P0-3）**：骨架常存；UI-1 起用留白与 Timeline 分层，不再画底部分隔线，缺字段只隐藏该项；branch 仅 GitDiffInfo.branch、有 active session 且无 session_mismatch 时显示（wire WorkspaceSummary 无 branch）；终态只画 live 可派生 Running / Needs input / Blocked（SessionLiveStatus 同源），wire 无终态字段不画 Completed 绿点；无 active Task 的空态由中央 Primary New task 承担唯一主路径，Header 同态不重复该动作。assistant 角色词 render 与 AX 统一为 Pawork；tool 行无耗时字段（wire 无 duration）；Run 摘要卡状态圆随终态种类（Completed 绿 ✓ / Failed danger ✕ / Cancelled —），不对失败/取消宣称成功；Review changes 走真实 Inspector/Changes 入口（先快照可用性再 refresh，Changes unavailable 时 disabled 给原因、Fetching 进行中不误报），Open in editor 无 capability 不画；消息/错误/页脚时间戳经 `display_time` 渲染为相对词 now/Nm/Nh/Nd（epoch millis 解析失败原样兜底，不伪造）。
- **重连三态可见**：Replay / SnapshotRequired / UpToDate 必须以文字在侧栏区分（不只靠颜色）；仅 SnapshotRequired 换基线重分页。
- **TaskRail 状态点诚实语义（R3 Wave A + Wave B）**：`SessionLiveStatus` 三态——NeedsInput（该 session 有待审批）> Running（`active_runs` 成员，含 live `RunChanged` 非终态登记）> Blocked（R3 Wave B live 派生：该 session 最近一条 `RunChanged` 为终态且 state ∈ failed / interrupted，completed / cancelled 不算；同 session 任何其它 `RunChanged` 清除；快照重建清空——wire 无终态来源，Replay 重放终态事件可重新派生）；其余会话一律空心灰圆，wire 无每会话终态字段故不画终态绿点；apply_event 在 active-session 闸门前跨会话维护成员关系，终态按 run_id 移除并清 pendings。unread 为独立通道（`session_unread()`）：非 active session 的 Session-stream 活动事件（RunChanged / AssistantDelta / ToolStarted / ToolOutput / ToolCompleted / MessageSent / Diagnostic；MessageSent 为本地 composer 回执只属 active session，不经 wire）标记，`select_session` 清除，首连 / 快照重建不产生（无 last-seen 基线）。
- **诚实显示**：Run tokens 仅取终态持久化累计用量，缺失为 `—`；底栏不再常驻 quota / tok/s 占位；ContextMeter 只用 catalog 的 `context_window_tokens`；Changes / Resources 未拉取显 unavailable 而非 0；`now_ms` 由 UI 注入，投影层不读系统时钟。
- **终端约束**：`terminal_create` 的 cwd 只接受 workspace 相对路径（拒绝绝对路径、Windows 盘符前缀、`..` 分量）；终端面为滚动纯文本，无本地 PTY/VT emulator。显示层与 AX 同源过滤 CSI/OSC 等 ANSI/VT 控制序列并归一 CR 换行，不修改 Host 保存的原始 output。输入草稿和 create 失败按 workspace 隔离；ADR-045 已提供真实 `terminal_close` 与 live `TerminalExited`（API 1.3，旧 minor 不推新事件），running 显示 Stop、已知 exited/killed/failed 显示 Close，不写入 `exit` 文本伪造生命周期；仅 exited/killed 可经 Start/New 直接走 `terminal_create` 重建（沿用 workspace 与可证 cwd，cwd 未知时回落工作区根），failed 表示 forwarder 断流且进程可能仍运行，必须先 Close 清理后再 Start；旧终端只读保留；新建终端初始尺寸取 `terminal_settings` 生效 columns/rows（ADR-050 D4：查询缓存，未查询到回落 80×24；投影初始值与创建后那次 resize 同源，resize 回执才确认）；尺寸变更只经 `terminal_resize`（stepper 本地草稿、apply 下发、在途去重、匹配回执/切换复位）；write/resize 瞬态失败在 runtime_state==running 时保留可写、仅 status_hint 报错；快照 cwd 为 Host 权威，缺/空键诚实显示 `unknown`。
- **Changes scope**：Host 的 `diff_list_files` / `diff_get` 均解析 latest session；UI 明示该 scope，且两次请求的 session id 不一致时 fail-closed 要求刷新，不能把新会话内容挂到旧列表。
- **心跳配比**：独立 15s 心跳任务对 host 30s 超时的节拍不可静默改动；断线不取消 Run。
- **窗口、字号与焦点**：默认 1440×1024、最小 1080×720（`WINDOW_MIN_SIZE`）；字体以 16px 根字号的 rem token 表达，100% 保持冻结视觉，125%/150% 只由应用快捷键调整窗口 `rem_size`，几何 px token 不随意缩放；消息正文 / 完成摘要行高以 24px 为 100% 基准并换算 rem，放大时避免多行正文负 leading。150% rail=320，1080 窗口仍保留 760px Workspace。macOS 透明 titlebar；启动与用户发起的任务切换、审批、Fork 后聚焦 Composer；激活当前 task 仍关闭菜单并聚焦 Composer；Review changes 展开 Inspector 后聚焦 Changes 选中页签；点击输入框显式拉回焦点。
- **Settings 外观页**：SET-6e 将上述三档字号暴露为始终可用的本地 Settings 页面；页面按钮、Cmd+=/Cmd+-/Cmd+0 和 AX Press 共享同一 `AppView.text_scale`，立即改变当前窗口 `rem_size`。ADR-053：与语言共同保存到 `desktop.json`，快捷键同入口；写成功后才更新显示，重启恢复，缺文件默认 100%。
- **界面语言（i18n，2026-09-05）**：默认 English，Settings → Appearance 可切换简体中文；全局 `AtomicU8` + 静态目录，切换即时重渲染（含 AX name / value / description），ADR-053 起保存到用户目录，重启恢复。render 与 AX 同源调用 `t()`；AX 节点 id、数据内容（session 标题、provider / model id、路径、工具输出、wire 错误原因）与品牌名 / 功能符号不翻译。不引入区域格式化工具链或第三语言；未知 key 原样回显。
- **Settings 高级页**：SET-6f 将当前连接已有的非 Secret 握手摘要、socket endpoint、resume/ack 暴露为始终可达的本地只读页；Connecting/断线清空 runtime/API/capabilities，Failed/Disconnected 复用既有 Reconnect。runtime ID 不称作 CLI `--instance` 配置名；页面不显示 GUI token/token path、不推断 data directory、不 shell-out `doctor`，也不提供实例切换。
- **Settings 关于页**：SET-6g 只在当前 Connected 握手提供非空 `host_data_dir` 时动态发布导航与只读页；三项值分别来自 Desktop 编译元数据、当前协商 API 和 Host 握手，render/AX 共用同一行模型。仅空白字段按缺失处理，但合法路径值原样展示；Connecting/断线清空握手并从 About 退回高级。不提供 updater/release/License 或任何写动作。
- **Accessibility 单一语义源（ADR-042）**：`AppView` 只从 canonical UI 状态与布局 metric 构建显式 `AxTree`；壳层几何与 render 共享 `shell_layout::resolve`（100% 窄窗 rail=240、150% rail=320），`composer-status-hint` 发布字号百分比；稳定 identifier 与本地化 label 分离，macOS bridge 只做 AppKit 映射。AX press / focus / set-value 必须回到既有 handler 与 enable gate，未知请求 fail-closed；disabled 控件不得发布可执行 action。Grouping AX Press 直接 toggle；其余触发器先移 GPUI 焦点再开菜单。Timeline AX 与 render 共享 rows + approval item 序列、tool group 折叠态和 Review gate；稳定帧读取真实 list bounds，首帧用共享公式回退，视口外条目不发布。Settings 普通树不得携带 API key 明文或 masked credential 片段；secure input 只发布等长掩码，stale 时输入与所有写动作 disabled 且无 Press。IME composing 中 AX Send 与键盘 Enter 同样不生效。新增可见交互须同批补节点、bounds、状态和 action 映射；非 macOS 当前为 no-op，不宣称已有平台 AX 实现。
- **平台显示偏好**：单一深色 palette 保持冻结 token，不读取系统显示偏好（macOS Increase Contrast 桥已于 2026-09-04 随功能移除）。当前 UI 无动画或过渡，因此 Reduce Motion 不需要分支。

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

UX-05 模型管理弹层：共用 `model_search_input` / query，仅过滤显示目录，不改变模型或批量启用语义；固定头部含搜索、状态 / 来源、完整目录计数、全开 / 全关。最大高 520px，内部模型列表独立滚动，名字与 ID 不截断并可横滚；↑/↓ 移 Switch 焦点，Tab 后实际聚焦行滚入，Esc 回管理入口。render / 键盘 / AX 使用同一筛选结果和实际列表框，保留所有入口写 gate 与 Host 回执权威。

UX-05 复用并更新两个现有主路径回归：`model_menu_ax_culls_rows_outside_clipped_frame` 覆盖 20 模型当前项可见、大小写 ID 搜索、Enter 选择不发送草稿、无结果恢复、键盘滚动 / AX 和 Esc 回焦；`settings_models_menu_ax_pins_gates_switches_and_empty_state` 覆盖原有写 gate / 全量动作与内部列表滚动，以及三档字号下名称 / ID 搜索、无结果恢复与实际可见 Switch。没有新增测试体系。真实窗口和候选证据见 [路线图 UX-05](../../ROADMAP.md#ux-05-模型选择与管理效率)。

UX-03 定向回归 `project_task_guidance_preserves_context_and_wraps`：项目筛选不重绑 / 不改草稿，项目新建菜单排除「所有项目」，Esc 回焦与第二次 Enter 确认，宽窄窗 × 三档字号下元信息实际框不重叠 / 不越界；有项目后移除限制与新建入口但保留其他反馈。复用既有 Composer 布局、会话草稿与目录菜单滚动测试；真窗口证据与用户验收状态见 [路线图 UX-03](../../ROADMAP.md#ux-03-新任务与项目上下文引导)。

## 7. 测试与验证资产

UX-06：复用并更新角色 / Provider AX 布局与三窗回归，覆盖连接优先排序、角色位置与未生效说明、实际滚动框、三窗来源 / 已用 / 剩余、剩余未知与过期；新增一个分钟精度倒计时主路径测试（含 6995 分钟 → 4d 20h 35m）。Desktop 224 项测试与 build 通过。真窗口已覆盖账号信息、不可用与刷新、宽窄窗三档字号、角色键盘导航；真实三窗数值 / 重置倒计时因当次查询不可用仍待补验，用户验收未完成，见 [路线图 UX-06](../../ROADMAP.md#ux-06-供应商与账号信息层级)。

UX-04 新增 `recovery_keeps_errors_local_and_preserves_drafts`：覆盖离线空态、诊断 / 重试、草稿保留、模型状态、只读创建阻止、实际拒绝的局部提示、权限未知与 stale gate、终端 I/O 错误及成功恢复；复用并扩充 `terminal_io_failure_keeps_running_terminal_operable`。本批 Desktop 223 项测试通过；真实 read-only 拒绝、连接恢复及三档字号证据与验收边界见 [路线图 UX-04](../../ROADMAP.md#ux-04-错误在发生处解释并提供下一步)。

UX-01 新增 `wrapped_draft_keeps_text_hit_testing_and_ime_in_sync`：三档字号、两档输入宽度下覆盖长中文、无空格路径、emoji / 组合字符与显式空行，核对完整原文、字宽、滚动、鼠标与 IME 坐标往返、组合更新及单次撤销。既有 Composer 实际布局 / AX 用例改用长段落，继续核对宽窄窗控件位置、卡片预算和断线禁发；测试与真窗口完成状态见 [活动路线图 UX-01](../../ROADMAP.md#ux-01-长草稿自然换行与编辑)。

焦点反馈回归 `pointer_focus_stays_functional_without_ring_and_keyboard_restores_it` 使用真实 GPUI 鼠标 / Tab 事件，覆盖点击保留功能焦点、键盘恢复提示、点击遮挡并吞事件的控件仍清除提示。

UI-2 新增 `session_actions_follow_hover_and_keyboard_without_opening`：真实 GPUI 鼠标 / Tab 驱动非当前会话动作，核对悬停可见、点击改名不切会话、取消后焦点保留、断线禁写与 AX 同源；本批实际命令与结果见 [路线图](../../review/roadmap-ui-2026-09-09.md)。

UI-4 本批实跑 216 个测试，全部内嵌于 bin target（`#[cfg(test)]` 模块；无 crate `tests/` 目录），下表保留历次测试资产分布，本批总数以实跑日志为准：

| 文件 | 数量 | 覆盖面 |
| --- | --- | --- |
| `main.rs` | 1 | `WINDOW_MIN_SIZE` 钉 1080×720 设计响应式底线（R2 Wave B） |
| `controller/mod.rs` | 18 | 既有 wire/解析/安全回归；R6 Wave B 增 terminal create 失败 workspace 归属与 diff 内容 session id 生命周期字段。SET-6d 增 terminal_settings 查询/全态写 wire 与回执解析主路径一条（shell Some/null 两态）。 |
| `projection/tests.rs` | 67 | 既有 snapshot/replay、Run/Timeline/TaskRail 与 Terminal 投影；Settings Data 走 protocol 类型 fail-closed（缺 nullable 键 / 类型错误）；SET-3～6d 解析与 stale/禁写回归仍在。 |
| `platform.rs` | 4 | socket / token 默认路径与 instance 命名；socket→token 推导；deny-list 恰为 `{pawork-client}`；扫描器覆盖别名 / target 表（负例含 dev-dependencies 排除） |
| `ui/settings/mod.rs` | 4 | SET-6d：空 shell Save 映射为 null、尺寸合法才可保存；OPT-3 角色区与 Manage models 弹层渲染 gate；角色控件 identifier 构造/解析互逆。 |
| `ui/mod.rs` | 17 | 既有键位、tab_stop、TaskRail 导航、Composer 与 per-session 草稿接线；进入/Refresh/重连 `refresh_all_settings`；R6 Wave A Header Activity；R6 Wave B Inspector 键盘目标、terminal gate 与 workspace 草稿/回执归属；R7 Wave C 钉字号放大/缩小/重置键位登记；P3 增已知 exited 终端 Start 重建 gate |
| `ui/accessibility.rs` | 3 | identifier 唯一与父子关系校验；focus 单一性；bounds hit-test 与无效树拒绝 |
| `ui/accessibility/settings.rs` | 1 | OPT-4d：100%/125%/150% 下导航 AX 与真实 render bounds 一致，同一项选中/未选中零位移 |
| `ui/accessibility/app.rs` | 21 | 稳定 identifier、TaskRail/Timeline/审批/菜单焦点与几何同源回归；Activity 320px 内容宽，内容高随字号为 144/180/216px 锚点；secure API key 只发布等长掩码、普通 provider summary 不发布 masked credential、provider 列几何与 render 全宽内容列同源（`SETTINGS_CONTENT_PAD` 32×2，OPT-4c），stale 后输入与写动作 fail-closed；model 菜单裁剪框外行不入树；OPT-4 审查新增 Scope 多项目菜单实测裁剪/滚动/键盘高亮主路径；本地 Settings 页覆盖 Advanced 离线导航、连接摘要、Reconnect gate 与 Appearance AX Press 150%；UI-2 悬停 / Tab 行操作不打开会话；UI-4 增实际 GPUI 布局与 AX 操作区对照（宽窄窗、三档字号、长草稿、断线禁发） |
| `ui/accessibility/macos.rs` | 6 | 顶左 bounds → AppKit parent space 坐标转换；value-change diff；结构骨架比较（属性变化不触发重建）；settable/action 双门拒绝越权 value / focus 写入；disabled action fail-closed（macOS） |
| `ui/barriers.rs` | 1 | timeline_stable 重写且 settle_seq 单调、字段形状齐全；approval_visible 写入（含 tool 名）与消失删除；未启用（None）零写入 |
| `ui/input_area.rs` | 4 | Composer placeholder / 面板高度、模型菜单分组与全禁用空态 |
| `ui/theme.rs` | 8 | UI-1 AA 文字 / 交互面对比度、TaskRail、Header/Timeline 与 Composer token；100%/125%/150% TextScale、P0 foundation spacing/radius/focus/menu token 与 Activity 320×144 基准 token 断言（缩放几何由 AX 测试覆盖） |
| `ui/shell_layout.rs` | 5 | UI-1 过渡反向连续 / 结束 / 窄窗归零；1280 阈值 rail 288↔240；1440×1024 三栏；1080×720 Inspector 折叠 + Workspace ≥560；同一解析测试另钉 150% rail=320 且 Inspector 保持折叠 |
| `ui/changes.rs` | 7 | ActivityPopover 摘要、非空可审阅文件 gate、二级页签、epoch/path/session 三重拒旧、断线 stale、latest-session mismatch 与真实横滚内容模型 |
| `ui/inspector.rs` | 3 | 顶层页签默认 Changes；Terminal 纯文本输出过滤 bracketed-paste ANSI/VT 控制序列并归一换行；P3 增尺寸 stepper 钳制 |
| `ui/resources.rs` | 3 | 默认 Idle、epoch 拒过期与断线保留旧数据但标记 stale |
| `ui/markdown.rs` | 2 | Markdown 块结构 / 行数估算；Unicode / 流式保留内容；UX-02 表格对齐、escaped pipe / 代码内 pipe、代码原始 CRLF 复制、HTTP(S) 与配对括号链接 |
| `ui/timeline_entry.rs` | 6 | R4 Wave A 纯逻辑：tool 状态词映射（仅 succeeded→Completed）/ 状态分类 / ToolRowView 构造；`display_time` epoch 串 → 相对词（now/Nm/Nh/Nd 边界）与非法串原样兜底 |
| `ui/text_input.rs` | 13 | 多行粘贴行计数；AX set-value 清 marked range；动态 placeholder；Composer 视口预算不破面板总高；Terminal 28–220 独立预算；shift 选择经 SelectLeft/SelectRight 真实 action；IME 经真实 EntityInputHandler 路径 commit 单次入栈且中间态不可 undo；80 行真窗口 overflow scroll（max_offset>0、视口 28–163、caret 滚入视口）；滚动态点击映回可见内容行；reset_text 恢复草稿且清 undo；SET-4 增 secure 掩码只含 grapheme 数量对应掩码字符且非 secure 不发布掩码 |
| `ui/u1_probe.rs` | 14 | R1 Wave C U1 spike 矩阵 + R5 Wave B SelectAll/Copy/Cut/Undo/Redo、IME commit 单次入栈（真实 EntityInputHandler 路径）、空输入不可发送、Wave B 键位（含 Shift-Enter）keystroke→keymap→action 链路；AX 仍不在本层覆盖 |

UX-02 另在 `ui/accessibility/app.rs` 新增一个 GPUI 主路径测试：正文 / 代码精确复制、实际菜单 AX、同 Run 后继闭合边界与断线禁用；本批 Desktop 总计 221 项通过。

**验证命令**：

```bash
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders
```

- `--bins`：本包是 bin-only（无 lib target），任务指南默认的 `--lib --tests` 匹配不到任何 target。
- `--features gpui/runtime_shaders`：gpui 默认构建在编译期调用 Metal 着色器编译器；开发机仅有 Xcode CLT 时缺 Metal Toolchain 会构建失败，runtime_shaders 把着色器编译推迟到运行时使本机可闭环。2026-09-04 P0–P2 UI Roadmap 后 Desktop bin 门禁为 187/187；Increase Contrast 移除（2026-09-04）后为 186/186。

本包 dev-dependencies 为 `tempfile`（workspace `3`，仅服务 `ui/barriers.rs` 的临时目录测试）与 `gpui` dev 条目（`=0.2.2` + `test-support` feature，R1 Wave C 起；仅测试构建启用 TestAppContext/VisualTestContext，resolver v2 下不进生产二进制闭包），均不计入生产 deny-list。

**功能测试用模型**：需要真实 Provider 的功能验证固定使用 `opencode-go / glm-5.3-flash`；仅当次 Host `--provider` / `--model` 覆盖，不改持久默认。详见 [verification.md](../verification.md) §2.1。

**运行时验证资产**：`--probe`（连接 + snapshot + 模型目录一行摘要）与 `--probe-smoke`（流式回合 / 切模型 / 审批 / 取消 / 两次断线重连持久化 / `disconnect_survive`），配合隔离实例（`--instance` + `PAWORK_DATA_DIR`）在真实 host 上冒烟。[scripts/ui-ax-dump.swift](../../../scripts/ui-ax-dump.swift) 与 [scripts/ui-key-event.swift](../../../scripts/ui-key-event.swift) 可做真窗口 AX / HID 取证。历史编排脚本与运行证据已移出仓库；新结论必须按当前源码与真窗口重建证据。

截至 2026-09-01，正式 Host/Desktop（无 fixture/seed/mock）已完成添加项目、真实 Provider 对话、审批写文件、Changes 与真实 Git 对照、PTY 命令、P1 多项目/会话归属双粒度重开、P2 六链路可靠性与 P3 三面板验收（含 ADR-045 Terminal 生命周期）。P3 隔离实例以 AX + `stty`/`pwd`/Git/SQLite/Host snapshot/ps 双证据验证尺寸变更、exited 重建、cwd 恢复、Changes/Resources 刷新与断线 stale；ADR-045 复验 Stop→无重连即时 killed（进程组击杀证据）、Close→复位 not started 且快照清空、`exit 7` 即时 exited、断线 stale 不回归；P4 片 3 消除 AX 树三处固定偏移几何（stepper / 审批卡 / Timeline）；P4 片 2F 修复并真窗口复验 D1（AXPress 菜单同源移焦与来源回焦）、D2（Timeline/approval 共享 item 序列，稳定帧读取真实 list bounds，AX frame 中心真实点击落盘）与 D3（历史早期工具不清 snapshot 当前审批，切走跳回仍恢复），当前 Desktop 定向门禁 160/160。系统 IME 真实 composing 后续已于 2026-09-05 补证通过（VoiceOver 签字已于 2026-09-04 按用户要求移出范围），P4 其余切片未完成。

2026-09-04 P0–P2 UI Roadmap 已完成源码实现与当轮 Desktop 自动门禁（移除 Increase Contrast / VoiceOver 门禁前 187/187，范围调整后 186/186；含审查修复：Settings AX 列几何与 820px render 列同源、model 菜单 AX 裁剪）。当前源码重新执行 `./scripts/pawork-desktop.sh build` 成功，并在正式 Host / Desktop 上逐张对照 P0–P2 阶段图：1440×1024 宽窗与含约 3px 窗框的 1083×723 最小窗布局、Inspector 折叠、Activity、Settings 八页、provider 分层、100% / 125% / 150% 字号，以及 direct toggle / 菜单焦点路径均通过，三张阶段图视觉签字为 PASSED。当日测试 Host 以临时参数使用 `opencode-go / glm-5.3-flash`，但连接失败；该历史阻塞已由下一条补证关闭。

2026-09-05 P0/P1 最终补证：在不修改持久 Provider/default/proxy 配置的前提下，正式 Host/Desktop 以 `opencode-go / glm-5.3-flash` 完成真实 streaming、tool group、assistant response、tracked `edit_file`、Changes、`Ready for review` / CTA、折叠 Activity 与 Approval Deny 全链路。真窗口同时发现并修复两处事实偏差：Ready 但空文件列表不再显示 Review CTA；`pawork-git` 的多文件 NUL `numstat` 解析恢复真实 `+A/−D`（实窗单文件补证 `+1/−0`）。系统拼音 composing 的首次 Return 只确认组合、不发送 Run。验证标记已移除；Desktop 186/186、`pawork-git` 57 个 lib 单测 + 5 个 parser contract 及正式 build 均通过。P0/P1 Human acceptance：PASSED。

2026-09-06 OPT-3 GUI 控件批次（对照 OPT-D 签字稿）：Manage models 启用弹层（单模型 Switch + Enable all/Disable all + 空目录诚实空态）、页首四默认角色区（候选 = 已连接且已启用，vision/search 只保存标注）、代理开关改 Switch（OPT-3c）、Composer 过滤禁用模型与全禁用空态。门禁 207/207（含弹层/角色区/Switch/空态 AX 钉板、角色 identifier 互逆与 OPT-4d 导航零位移）。真窗口经 AX + 截图核对角色菜单分组（含 240px 折叠滚动）、Composer 与启用弹层；验收中经真窗口+协议层发现并修复 Host 侧清除判定误删盘上默认对缺陷（ADR-055 D3a，pawork-app 208 lib 含回归），协议层复验 Enable/Disable-all 与 cleared_roles 口径通过；代理 Switch 像素级复验已通过（见 [ROADMAP §10.6](../../review/roadmap-opt-2026-09-05.md#106-本批交付与证据2026-09-06opt-4-工作台与-settings-壳层落地)）。详见 [ROADMAP §10.5](../../review/roadmap-opt-2026-09-05.md#105-本批交付与证据2026-09-06opt-3-gui-控件批次)。

2026-09-06 OPT-4 工作台与 Settings 壳层：六处主操作命中区 ≥36×36、可见字形 20px；Inspector 默认折叠并在 Header 增加 inspector-expand 重开入口；Settings 内容取消 820px 上限（两侧 32px padding，render/AX 同源）；导航选中/未选中文字坐标零位移。门禁 207/207。首次真窗口对照新图验收已通过（[ROADMAP §10.7](../../review/roadmap-opt-2026-09-05.md#107-本批交付与证据2026-09-06opt-4-真窗口对照新图验收)）。详见 [ROADMAP §10.6](../../review/roadmap-opt-2026-09-05.md#106-本批交付与证据2026-09-06opt-4-工作台与-settings-壳层落地)。

2026-09-04 按用户要求移除 macOS Increase Contrast 支持与全部 VoiceOver 验收门禁：删除 `ui/platform_preferences.rs`（NSWorkspace 显示偏好桥），`theme::dark()` 回归单一冻结 palette 并移除对应定向测试（Desktop 门禁 187→186），Appearance 页主题说明同步更新；AX tree 与键盘支持保留。

2026-09-07 OPT-4 完成效果审查：修复 Settings 导航在大字号下 AX 间距漂移、外观页字号/语言按钮仍用旧原点与估算纵坐标、Scope 长菜单 AX 未裁剪且键盘高亮不滚动三项缺口。导航沿用 rem 布局，外观按钮和 Scope 菜单经宿主 `ScrollHandle` 读取 GPUI 实测位置；Scope 使用 `MenuPanel::track_scroll`，首帧布局后滚入当前项、键盘移动时滚入高亮项；AX 在布局完成后同步。复用导航/外观测试，新增一个 Scope 多项目滚动主路径。验证状态见 [ROADMAP §10.12](../../review/roadmap-opt-2026-09-05.md#1012-opt-4-完成效果审查与修复2026-09-07)。

2026-09-07 UI-1 已实现；Desktop 定向测试 213/213 与构建通过（本机使用临时库搜索索引，未改 Cargo 配置）。新增覆盖开合反向连续、窄窗归零、过渡期 AX 裁剪与不可见动作拒绝；共享文字对比度改按 AA 下限验证。代理真窗口检查通过：真实 Host 连接、Inspector 开合 / 连续切换与三页签、Activity → Changes、宽窄布局及 100% / 125% / 150% 字号；probe 确认 1 个空测试会话与 13 个模型，未发起 Run。用户人工视觉验收已通过（2026-09-07），未归档；证据见 [ROADMAP UI-1](../../review/roadmap-ui-2026-09-09.md#ui-1-本批证据2026-09-07)。

UI-6a 将目录 context window 的 0 哨兵视为 unknown，在 Context 显示 unavailable；复用既有投影用例覆盖该值。复用四个既有 Provider AX 测试，以真实 GPUI prepaint 对照元素框；覆盖 secure input 脱敏、角色过滤、模型启用与空目录、stale 禁写、角色长目录的当前项 / 上下键滚入、模型菜单滚动保持 / 离屏裁剪，以及 1080×720 下 100% / 125% / 150% 展开卡页面滚动。Provider 页面、四角色与模型弹层沿用 `settings_element` 实测框；无新依赖或 wire 变更。规格与验证分别见 [GUI 设计](../../gui-design.md#ui-6a-供应商更新2026-09-08) 和 [路线图](../../review/roadmap-ui-2026-09-09.md#ui-6a-本批证据2026-09-08)。

## 8. 注意事项与已知限制

- **gpui 前台执行器无 tokio reactor（历史崩溃教训）**：在 `cx.spawn` 的前台执行器上 await client 调用，会在 `receive_frame` 内部的 `tokio::time` 直接 panic。连接期握手 / ack / `subscribe_all` 与事件泵**必须**全部跑在 `runtime.spawn` 上，gpui 侧只经 channel 消费结果。`--probe-smoke` 走 `platform.block_on` 自带 runtime，暴露不了这类回归，因此生产窗口启动仍是必需门禁。
- **Changes 面只读**（用户拍板 2026-08-24）：git_stage / HunkStageService 接线顺延 ADR 候选；`@` 补全浮层与「已加载规则」分区无 Host 出口（`@` 端到端展开在 host 侧 crates/app，不在本 crate）。
- **host `diff_*` 固定解析 latest 会话**：数据会话与当前查看会话不一致时，UI 以 banner「Showing changes for latest session X — not the active session.」与 popover 提示行如实标注，不静默张冠李戴。
- **渲染面自动门禁尚未完整**：现有布局、主题、键盘与 AX 定向测试不覆盖完整 Timeline/Changes AX 几何与全组件 hover/inactive，性能阈值也未冻结；三张阶段目标设计图的当前构建真窗口视觉签字已通过，但不扩张为这些未覆盖项的自动保证。
- **ActivityPopover 已完成视觉签字**：divider/raised Changes section、仅有 Changes capability 时的 320px 内容宽，内容高随字号为 144/180/216px 收缩、最小窗锚定与相关 AX 子节点已由定向测试和当前构建真窗口共同核对。VoiceOver / 完整 screen-reader 已移出当前 GUI 验收范围；后续 capability 增减仍须重新验证动态内容高度与 AX 子树。
- **环境性断连**：显示器休眠 / App Nap 下心跳超时断连（Reconnect 横幅恢复）为宿主环境行为，非缺陷。
- **早死 run 的回显行重选后消失（R4 Wave B 评审 P3，存量语义）**：plan 闸门在 `MessageCommitted` 之前拒绝时，用户消息从未持久化；乐观回显让用户先看见消息，重选 / 重连后快照重建时该行随基线清空消失。消息此前根本不显示，echo 只是使该语义可观察；是否把用户消息持久化提前到闸门之前属 [产品候选](../backlog.md)。同理，合成兜底条目（≥2^60 序号）在屏时若同会话又有真实事件到达，真实事件按序号插到合成条目之前（深边角化妆性排序），重选即自愈。同一 run 的乐观回显行与稍后到达的持久化 UserMessage 在未经重选/重连时理论上可并存（echo 不进 seen）；实际触发面极窄——最新用户消息只经快照到达而快照会重建 timeline——重选即自愈。
- **单主题**：仅深色 `dark()`，不读取系统显示偏好（Increase Contrast palette 变体已于 2026-09-04 移除）。SET-6e 外观页只读陈述这一事实，不提供 light/system/custom theme 控件；`Theme: Global` 是未来运行时主题挂载点，当前未 `set_global`。
- **文件尺寸口径**：`ui/mod.rs` 约 4075 行；这是工程结构口径，不构成新 UI 视觉或交互放行条件。
- **`text_input.rs` 血统**：改自 gpui 0.2.2 `examples/input.rs`（Apache-2.0）。R5 Wave B 已对照上游补齐 Copy / Cut / SelectAll / 拖选 / Undo / Redo / overflow scroll；ShowCharacterPalette 仍裁剪。
- **FollowScroll 的滚轮时序假设**：`on_scroll_wheel` 直读已应用（未钳制）的 offset——依赖 vendored gpui 0.2.2 的 Bubble 相监听逆序分发（内部偏移应用先于用户监听）；升级 gpui 时须重核，做 delta 投影会把增量计两次。
- **UI fixture barrier 钩子（R1 Wave B，测试专用）**：启动读 env `PAWORK_UI_BARRIER_DIR`（main.rs；空值视同未设置，`--probe` / `--probe-smoke` 不发射）。未设置时全程零开销：不 spawn tick、无任何文件 IO。设置后由 ui/mod.rs 既有 1s tick 兼任发射点：已连接 && 无进行中 timeline 分页（`open_session` 置位、complete / `open session` 失败 / `Disconnected` 复位）&& 本 tick 窗口无 ControllerEvent 时重写 `<dir>/timeline_stable`（JSON 含 settle_seq 单调自增 / session_id / entry_count / at_ms / detail）；开始连接、打开会话或收到任一 ControllerEvent 时先删除旧 `timeline_stable` 与 `approval_visible`，防只等存在性的 driver 误收陈旧信号。`pending_approval` 存在且已稳定 → 重写 `approval_visible`（含 tool 名），消失 → 保持删除；目录不存在时由 `BarrierSink::new` 惰性创建。写入 tmp+rename 原子替换、任何 IO 失败静默跳过；`projection/` 保持纯状态机零 IO。controller 在未连接、无 TimelinePage 响应或翻页达到上限时发 `SessionOpenFailed{session_id}`，UI 仅在该 session 仍为 active 时复位分页状态。
