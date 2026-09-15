# Pawork 活动路线图：GUI 第二阶段

> 更新：2026-09-15；规划基线：`main / 7a416dd6`，当前主干 `8952a743`。2026-09-13 [ADR-061](spec/settings.md#adr-061账号默认名称与重命名2026-09-13) 将 Settings 账号改为默认名（邮箱 / API key 脱敏串）+ 可重命名，Go 三窗用进度条表示已用百分比；GUI API 1.17。2026-09-14 代理完成 §2.2 全部真窗口验收（V-01～V-12 与 ADR-061，证据 /tmp/pawork-vfix-evidence/）；同日下午补齐非空 MCP（GUI 内成功执行）与 GUI3-04 401 CTA（§2.2 末尾补录，含一次凭证操作事故登记，需用户重录 DeepSeek key），用户视觉验收仍未做。本阶段以 [PI-Desktop Screens](https://pi-docs.aiuo.net/guide/screenshots) 为交互与视觉参照，优化已有 GPUI 工作台。**GUI2-01～06 已实现并完成定向自动检查，等待用户验收；GUI2-02 真窗口限制见下文；GUI2-05 / 06 代理真窗口随 GUI2-07 批次完成；GUI2-07 于 2026-09-14 完成真窗口验收并同批复验 §4 GUI3 呈现（证据 /tmp/pawork-gui2-07-evidence/）**；生产能力以源码为准，下一阶段规格见 [GUI 设计](gui-design.md)。2026-09-12 完成一轮显示效果 Review，结论与新增的 GUI3 视觉任务见 [§4](#4-显示效果-review2026-09-12与-gui3-视觉任务)；同日 GUI3-01～07 已由子代理实现并通过 Desktop 定向测试（主代理逐 hunk 审查），代理真窗口 2026-09-14 随 GUI2-07 批次复验（01～03、05～07 通过，04 的 401 CTA 当日下午补齐、08 动效留人工），用户验收未进行；GUI3-08 已按 PI 暗色截图确认有限动效（不循环）并落地，定向测试见本轮报告。2026-09-13 用户在本机 `desktop` 真窗口走查当前工作区候选，疑似问题记 [§2.1](#21-用户视觉走查发现2026-09-13)。2026-09-15 工作区落地 GUI4 壳层收口：StatusBar 三栏（项目 / 分支 · 用量 · 反馈或连接）与共享 `EmptyState`（首页 / Changes / Resources），字号与操作提示改落右栏，Composer 不再承载瞬态 hint。

上一条 UX-01～UX-09 已有实现，详细证据迁至 [历史记录](review/roadmap-ux-2026-09-10.md)，未完成验收继续列于 §3。迁存记录不代表验收通过或任务归档。更早 UI-1～UI-6 见 [模块重设计记录](review/roadmap-ui-2026-09-09.md)。

## 1. 本阶段结果与范围

让用户进入窗口就能开始任务，长对话中容易找到内容，需要时直接查看变更或终端，设置项可查找且生效范围清楚。延续已有项目、会话、输入、Markdown、模型与账号能力；不把已经实现的 UX 修复再列成待开发功能。

参考页的价值落在清楚的空间分工：侧栏负责找到任务，主区负责阅读与输入，工作面板按需出现，设置是独立目的地。Pawork 本阶段保留 Rust / GPUI、深色主题与既有能力边界；PR、定时任务、插件市场、浏览器预览、文件浏览器、浅色主题不随参考截图进入实施范围。具体取舍和原图见 [参照映射](gui-design.md#2-参照与取舍)。

**实施顺序**：GUI2-01 → GUI2-02 → GUI2-03 → GUI2-04 → GUI2-05 → GUI2-06 → GUI2-07。每项以单一界面或紧相关模块收口；遇到需演进协议的需求，先拆出契约设计，不扩大该界面任务。

| 任务 | 交付结果 | 依赖 | 实现 | 自动检查 | 代理真窗口 | 用户验收 / 归档 |
| --- | --- | --- | --- | --- | --- | --- |
| GUI2-01 | 精简工作台壳、首页与侧栏 | 无 | 已实现 | 227/227 通过 | 本项路径通过，见下方限制 | 待验 / 未归档 |
| GUI2-02 | 对话阅读层级与 Composer 整理 | 01 | 已实现 | 228/228 通过 | 部分通过，见下方限制 | 待验 / 未归档 |
| GUI2-03 | 任务与操作快捷查找 | 01 | 已实现 | 230/230 通过 | 本项路径通过 | 待验 / 未归档 |
| GUI2-04 | 当前对话查找与回合定位 | 02 | 已实现 | 231/231 通过 | 本项路径通过 | 待验 / 未归档 |
| GUI2-05 | 工作面板与窄窗可达性 | 01 | 已实现 | 243/243 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI2-06 | 设置查找与行式信息组织 | 03 | 已实现 | 246/246 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI2-07 | 受影响主路径验收与旧缺口收口 | 01～06 | 完成 | 未运行（无新测试） | 通过（2026-09-14，见下文） | 待验 / 未归档 |

## 2. 可独立交付的任务

### GUI2-01 工作台壳、首页与侧栏

- **当前基础**：已有 288 / 240 / 320px 侧栏、项目筛选与两种分组、44px 任务行、归档撤销；顶部标题、筛选、连接分占多行，Header 为 80px。
- **改动**：按 [壳层规格](gui-design.md#4-工作台壳与首页) 收敛侧栏顶部为两行，把健康连接状态移到底部 Local 区；Header 目标为 64px 起、随内容增高。无任务与空任务共用首页说明，Composer 留在底部；创建入口保留全局无项目与项目内新建的区别。
- **写入集**：`apps/desktop/src/ui/{mod,task_rail,timeline,shell_layout,theme,i18n}.rs` 及对应 AX；按实际改动更新 [Desktop 包级 Spec](spec/crates/desktop.md)。
- **验收**：有任务、无任务、空项目、无项目、断线五种状态均给出真实下一步；筛选不改归属，取消目录选择不创建记录；改名 / 归档可达且能撤销。1440×1024 与 1080×720、三档字号下无控件遮挡，traffic lights 安全区保留。
- **最小切片**：先壳层与侧栏，再首页；各切片定向验证后即可交付。

**本轮进展（2026-09-10，`main / e4d0b752` 工作区候选）**：侧栏收为「任务 / 新建」与「筛选 / 分组」两行，连接状态移到底部；Header 最小 64px、内容可撑高；无任务与空任务共享首页标题，空任务直接引导底部输入，加载历史或运行时不显示欢迎态。键盘前缀调整为新建 → 筛选 → 分组；相关控件与首页的 AX 使用实际布局。搜索按钮与真实查找能力一并留在 GUI2-03。Desktop 定向测试 227/227 通过（含新增首页 / 宽窄字号布局回归），Desktop 候选构建通过。用户验收与归档未完成。

- **候选与命令**：`main / e4d0b752` 加本轮工作区；`env -u CARGO_MAKEFLAGS cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`、同参数 `cargo build`。独立 `Pawork-GUI2-01.app` / `gui2-01`，二进制 SHA-256 `5e6117af5303ec4d0b905e03243ef4dc288980dd2344e1be620fb55fdbdbf2fa`。宏动态库加载异常缓慢，首次编译中止后重跑；最终命令均 exit 0。模型审查工具因路由错误未启动，主代理完成差异审查。
- **代理真窗口**：1440×1024 初始窗口与 1080×720 最小窗口，100% / 125% / 150% 下壳层、侧栏与首页无控件遮挡；36px traffic lights 安全区保留。新建 → 空任务引导、改名、归档 → 撤销恢复相同任务及草稿、空项目筛选不改变无项目归属、取消目录选择、项目内新建均已操作。键盘确认新建 → 筛选 → 分组顺序及 Esc 回焦；停止隔离 Host 后正文 / 草稿保留，窄窗 100% / 150% 重试入口可达，重启 Host 并点击重试后历史 / 草稿恢复。
- **源码外事实**：Host SQLite 只读核对两个会话：`ses-1789027191082-1` 改名后恢复、`archived=0` / `workspace_id=null`；`ses-1789027302509-2` 绑定 `ws-default`。目录取消后仍仅一个原有 workspace。截图、AX、会话导出及只读存储摘要位于 `/tmp/pawork-gui2-01-evidence/`；测试 / 构建 / Host 日志为 `/tmp/pawork-gui2-01-{tests,build,host}.log`，均未检入仓库。
- **限制**：指定 `opencode-go / glm-5.3-flash` 的真实请求已发送，终态为 **HTTP 401**，界面与持久会话均显示失败。只证明非空任务与错误呈现 / 重连恢复，不作为成功 Provider 对话、工具或 usage 证据；GUI2-07 与 §3 的这些缺口保持待验。

### GUI2-02 对话阅读层级与 Composer

- **当前基础**：已有 880px 阅读列、Markdown 表格与代码复制、思考 / 工具默认折叠、单一终态页脚、自然换行及逐任务草稿、可搜索模型菜单。
- **改动**：按 [对话规格](gui-design.md#5-对话阅读与运行状态) 将短用户消息改为靠右内容宽气泡，助手正文开放排版；动作低强调但键盘可达。按 [输入规格](gui-design.md#6-composer) 保留底部模型唯一入口，梳理模型、项目、上下文与本轮用量的层级。
- **写入集**：`ui/{timeline,timeline_entry,input_area,theme,i18n}.rs`、相关 `ui/mod.rs` 装配与 AX。只有实际编辑行为有缺陷时才改 `text_input.rs`，不重写输入引擎。
- **验收**：短句、长中文、代码、表格与长链接不裁切；展开不丢阅读位置，流式期间上滚不被拉回；复制仍与原文一致。模型搜索 Enter 不发送草稿，IME composing Return 不启动 Run，三档字号下输入与动作可达。
- **最小切片**：阅读排版、Composer 整理分别实施；视觉调整不改持久事件或 reducer。

**本轮进展（2026-09-10，`main / 35ca6125` 工作区候选（最终 bundle `Pawork-GUI2-02-verified2.app`，SHA-256 `60af2ae9e0eb6e0bafc1594a8cb71d638c977f76da34170e7d8399ebfc8fc826`））**：短用户消息按内容收缩并右对齐，普通正文最多占阅读列 80%，代码 / 表格可用整列；气泡采用 12px 纵向 / 16px 横向内边距，用户时间与菜单移至气泡外。助手保持开放排版，消息动作悬停 / 聚焦时显示。模型菜单固定提供管理导航，空目录 / 无结果仍可进入 Providers；进入后焦点留在根节点，避免同一 Return 立即触发返回。空闲回收底部运行栏，历史用量继续在每轮页脚呈现。没有修改输入引擎、事件持久化或 reducer。

- **自动检查**：Desktop 定向测试 228/228、候选构建通过。新增一个阅读 / 导航主路径回归覆盖宽窄三字号短气泡、动作 AX 实测框、空目录管理导航与草稿保留；既有复制、模型搜索、IME、折叠与滚动测试通过。命令为 `env -u CARGO_MAKEFLAGS cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 及同参数 `cargo build`；本机额外使用 `/tmp/pawork-gui2-02-rustc-wrapper.py` 搬移宏动态库、`/tmp/pawork-gui2-02-test-runner.py` 从临时目录启动测试，复用原 target 增量缓存。模型审查因工具路由错误未能启动，主代理自查差异。
- **代理真窗口**：1440×1024 / 1080×720、100% / 125% / 150% 检查短消息、长中文、开放助手正文、代码 / 表格 / 长链接和输入动作可达性；短消息与长代码经真实复制后粘回草稿，原文保留。模型搜索 Enter 只选择模型，无结果时 Enter 可进入真实供应商页，返回保留草稿。工具 fixture 展开保持所在行的阅读位置；长代码经滚轮到达行尾（`";` 完整可见）后不再移动。布局 fixture 仅用于呈现与状态覆盖。
- **真实请求与限制**：通过当前正式 Host，以 `opencode-go / glm-5.3-flash` 发出真实请求，会话 `ses-1789035615664-1` 的持久事件明确记录模型与 **HTTP 401 / authentication** 失败，界面终态一致。成功流式及上滚脱钩、成功工具 / 非零 usage 保持待验。系统键盘输入未形成可观测的 IME 组合态，不能据此关闭 UX-01 的系统 IME 缺口；既有自动 IME 测试通过不等于系统验收。
- **证据**：截图、AX、候选散列、布局原文与真实请求持久事件位于 `/tmp/pawork-gui2-02-evidence/`；测试 / 构建日志 `/tmp/pawork-gui2-02-{tests,build}.log`，正式布局 Host 日志 `/tmp/pawork-gui2-02-layout-host.log`。用户验收与归档未完成。

### GUI2-03 任务与操作快捷查找

- **差距**：当前只有模型搜索、任务循环与关注任务跳转，没有 `Cmd+K` 任务 / 页面查找。
- **改动**：增加一个 [快捷查找浮层](gui-design.md#7-查找与定位)，搜索当前 Snapshot 中的任务标题 / 项目名和已有页面 / 安全导航操作；显示所属项目并区分同名任务。入口为侧栏搜索按钮和 `Cmd+K`，初始展示最近任务。
- **写入集**：Desktop `ui/` 中一个局部查找模块、`ui/mod.rs` 动作 / 焦点装配、TaskRail、i18n 与 AX；复用打开任务 / 设置 / 面板 handler。
- **验收**：中文、英文大小写、同名不同项目、无结果、列表滚动与 Esc 回焦；选择任务才切换，输入查询不改草稿，跨筛选打开时解除遮挡并说明。断线仅允许本地设置 / 诊断动作；不误派写命令。
- **边界**：范围标为当前任务列表；不读取 SQLite、不遍历所有历史正文、不宣称能发现窗口重启前的归档任务；审批、凭证操作、删除、Run 启动不进入快捷执行项。

**本轮进展（2026-09-10，`main / c15f6816` 工作区候选（最终 bundle `Pawork-GUI2-03-final.app`，SHA-256 `be9ef3b549272022202e9e664bdb7a58dd6d61c5e86213607abb2e80eb5c991a`））**：侧栏任务行加入搜索按钮，与 `Cmd+K` 打开同一浮层。初始按更新时间降序列出当前 Snapshot 任务并标注项目 / 无项目；查询忽略英文大小写、匹配标题与项目名，结果按任务 / 页面 / 导航操作分组。选择任务走既有 open_session，解除遮挡它的项目筛选并提示、展开对应项目；页面与面板只调用既有 Settings / Inspector handler。查询框独立于 Composer，输入不切任务、不改草稿；浮层内方向键滚动高亮、Esc 回到原控件，模态期间隔离底层审批 / 新建 / 检查器等写操作快捷键与 AX。断线后仅保留本地外观与高级诊断入口，执行时重新核对当前结果，拒绝失效节点。浮层最多 640px 宽 / 视口 60% 高，少量结果随内容收缩；AX 只发布可见结果的实际裁剪框。中文界面占位提示经真窗口发现为英文后已修复。

- **自动检查**：Desktop 定向测试 230/230、候选构建通过。新增 `quick_find_navigation_layout_and_drafts`（任务排序、同名不同项目、中文与大小写、无结果、键盘滚动、宽窄三字号布局、Esc 回焦、跨筛选导航与草稿保留）与 `quick_find_disconnect_rejects_stale_actions`（断线结果收敛、陈旧任务 / AX 动作拒绝、写操作快捷键隔离）；既有 228 项回归全部通过。命令为 `env -u CARGO_MAKEFLAGS cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 及同参数 `cargo build`；本机复用 `/tmp/pawork-gui2-03-rustc-wrapper.py`（宏动态库从 /tmp 副本加载）与 `/tmp/pawork-gui2-02-test-runner.py`，复用原 target 增量缓存。首次编译复现宏动态库加载停滞，采样确认后换 wrapper 通过。模型审查工具因路由错误未启动，主代理完成差异审查。
- **代理真窗口**：1440×1024 与 1080×720、100% / 125% / 150% 检查浮层标题、提示、分组与结果无遮挡，长任务列表方向键滚动至 Task 31，无结果 Enter 不切任务且浮层收缩。侧栏按钮与 `Cmd+K` 双入口可用；中文项目名搜索命中、英文大小写匹配；同名任务以项目区分。跨项目筛选中选择 Beta 任务后筛选解除并提示，返回原任务后草稿完整保留；设置页经查找往返。停止隔离 Host 后结果仅剩外观 / 高级，浮层内 `Cmd+N` / `Cmd+I` / `Cmd+Return` 均无动作，进入高级诊断页并返回后草稿保留；重启 Host 点击重连后任务结果恢复。
- **源码外事实**：验收用隔离数据目录 `/tmp/pawork-gui2-03-layout`（GUI2-02 布局库副本加 40 条空搜索 fixture 会话，清单见 fixture.json），未发真实 Provider 请求，也无须发。截图、AX、候选散列位于 `/tmp/pawork-gui2-03-evidence/`；测试 / 构建 / Host 日志为 `/tmp/pawork-gui2-03-{tests,build,host}.log`，均未检入仓库。
- **限制**：浮层只覆盖当前 Snapshot，重启前归档任务与历史正文不在范围内（按边界设计）；用户验收未完成。

### GUI2-04 当前对话查找与回合定位

- **差距**：已有虚拟列表和回到底部，尚无正文查找及回合目录；PI 的 minimap 提供了快速定位的参考。
- **改动**：先实现 `Cmd+F` 在当前已加载的用户 / 助手正文中查找、上下命中切换；再以用户消息摘要组成回合目录，点击定位到该回合。使用稳定事件 / 消息身份重新求当前行号；本阶段不做悬停放大 minimap。
- **写入集**：`ui/timeline.rs`、局部查找 / 定位状态、`ui/mod.rs`、i18n 与 AX；保持现有分页与共享投影。
- **验收**：历史加载期间标明范围；新流式、工具折叠、宽度变化后仍指向同一消息；切任务清除命中状态，不跨任务跳错。定位进入阅读模式，显式回底才恢复跟随；无结果不触发全库扫描；关闭后恢复入口焦点。
- **最小切片**：正文查找先交付，回合目录后交付；内容超过一屏且有多个回合才显示目录入口，短会话不增加常驻栏。

**本轮进展（2026-09-11，`main / ea6d9d09` 工作区候选（最终 bundle `Pawork-GUI2-04-final.app`，SHA-256 `1c444be5779f4ab36f9022fc35160a7ac3159e0330a99071db61a9a61d426c95`））**：Header 新增「查找 · ⌘F」入口（有任务时常在）与「回合」入口（多个用户回合且内容超出一屏才出现，短会话不显示）。`Cmd+F` 打开工作台内联查找栏：只查当前已加载用户 / 助手正文，忽略英文大小写，计数以匹配消息为单位；↑ / ↓、Enter / Shift+Enter 循环定位，当前命中整条消息以背景标识。历史分页未完成时范围说明为「历史加载中 · 仅查找已加载正文」；思考、工具输出与其他任务不参与，无结果不访问 Host。定位键使用协议消息 ID + run_id，事件 ID 兜底：流式 delta 转 committed、本地 echo 转持久用户消息后身份保持，渲染前重求虚拟行号，行插入 / 工具折叠 / 宽度变化不缓存旧位置。回合目录按用户消息摘要排列、列表可滚动，选择后关闭并定位；跳转进入阅读模式，手动滚动解除锚点但不自动回底，显式回底才恢复跟随。关闭查找恢复入口焦点，切任务清空查找与命中状态。协议、共享投影与分页机制不变。

- **自动检查**：Desktop 定向测试 231/231、候选构建通过。新增 `conversation_find_identity_reading_and_turns` 覆盖已加载范围标注、中文与英文大小写、无结果禁动作、命中循环、宽窄（1440×1024 / 1080×720）× 三档字号实测布局、行插入后身份重映射、真实 reducer 的流式转提交、Esc 回焦、草稿保留与切任务清空；既有 230 项回归全部通过。命令为 `env -u CARGO_MAKEFLAGS cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 及同参数 `cargo build`；复用 `/tmp/pawork-gui2-03-rustc-wrapper.py` 与 `/tmp/pawork-gui2-02-test-runner.py`。
- **代理真窗口**：1080 宽候选窗口（显示输出限制为 1080×768）连接隔离 Host（`PAWORK_DATA_DIR=/tmp/pawork-gui2-04-layout/data`，模型 `opencode-go / glm-5.3-flash`，未发真实请求）。长对话样本（16 回合）中查找「第 1」命中 16 条、Enter / Shift+Enter 循环并实际滚动到第 1 / 第 2 轮；回合目录键盘滚动至离屏第 6 回合并正确定位；无结果「0 / 0」且上下按钮禁用；Esc 关闭恢复输入焦点、Composer 草稿保留、视口不回底。两回合样本（`GUI2-04 locate final`，8 条历史事件）验证跨回合 4 命中切换与 Shift+Enter 回到 1/4。150% 字号下查找栏控件无遮挡；短会话目录入口正确隐藏。
- **源码外事实**：隔离数据为 GUI2-02 布局库经 SQLite backup 复制到 `default/` 实例目录，另加确定性历史事件样本；截图与 AX 文本位于 `/tmp/pawork-gui2-04-evidence/`；测试 / 构建 / Host 日志为 `/tmp/pawork-gui2-04-{tests,build,host}.log`，均未检入仓库。
- **限制**：正文内精确字符高亮按设计不在本切片；查找不覆盖未加载分页（范围文案如实标注）；用户验收未完成。

### GUI2-05 工作面板与窄窗可达性

- **当前基础**：已有 Changes / Terminal / Resources、默认折叠、Activity 摘要与 Review 入口；宽度不足时强制折叠，显式打开也无法查看完整面板。
- **改动**：按 [工作面板规格](gui-design.md#8-工作面板) 保留宽窗 440px 侧面板；窄窗显式打开时在中央区域呈现同一个面板，并提供返回对话。以面板名称选择器 + 关闭动作替代固定三页签头，减少多级工具栏。
- **写入集**：`ui/{inspector,shell_layout,changes,resources,mod,i18n}.rs` 与对应 AX；Terminal 内容只改装配，不改 PTY 生命周期。
- **验收**：三个面板在最小窗口及 150% 下可达；返回对话恢复任务、草稿、阅读位置与 Run。开合 / 变宽 / 变窄不重建终端、不发送取消。Changes 仍只读且保留 latest-session 来源差异，断线仍标 stale；空面板不自动打开。
- **最小切片**：先面板头统一，再窄窗中央呈现；复用已有实体和数据，不复制面板实现。
- **显示效果补充（2026-09-12 Review，见 §4 R-04）**：面板头统一时同批收口 Changes 的可读性——从 hunk 头 `@@ -a,b +c,d @@` 本地递推左右行号（`DiffLineDetail` 只有 `kind`/`text`，不改 wire）；addition / deletion 改为整行专用浅底 diff token，不再复用 Allow 按钮的 `success_bg` 绿；文件清单由 44px `ListRow::task` + `▧` 改为 28–32px 紧凑行与 `M / A / D` 状态色标；修正 `inspector.rs` 模块注释与实现（24×2 中性下划线）不一致处。验收补：行号与 gutter 外背景经 `changes.rs` 既有测试钉住。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：`shell_layout.rs` 新增 `InspectorPlacement { Hidden, Side, Center }` 与 `can_fit_side_inspector`——偏好关 → Hidden；默认字号宽 > 1279 且 ≥ 1288（150% 需 ≥ 1320）→ Side（440px 右栏，`InspectorMotion` 180ms 仍只用于并排）；否则偏好开 → Center；`InspectorMotion::snap` 跨窄窗切换直接落位不动画；`inspector_open` 现表示 Side。`inspector.rs` 面板头统一为 48px「面板名称选择器（Dropdown 触发器 `inspector-panel`，当前名 + `▾`，菜单 `inspector-panel-menu` 列 Changes / Terminal / Resources，`MenuKind::InspectorPanel`，↑/↓/Enter/Esc 走既有菜单键盘语义）+ 36×36 `inspector-collapse` 关闭」，取代 UI-1 三页签头（`inspector-tabs` TabGroup 移除，`inspector-tab-*` 改为菜单项、仅菜单打开时发布）；Changes 内 Files / Summary 二级页签保留；Terminal 只改装配。中央模式：TaskRail 保留，同一 `inspector_element` 以 `Panel::fill()` 占中央 Workspace，顶部 36px「返回对话 / Back to conversation」`inspector-back`；不渲染 Timeline / Composer 且 AX 不发布，但保留实体、草稿、`timeline_following` / ListState 偏移与连接状态；返回 / 关闭回 Composer 焦点；`pending_approval` 时面板顶部显示「返回对话处理审批」`inspector-approval-hint`（点击即返回，不默认批准）。Activity / Review changes 打开面板时仅 Side 才 `timeline_changed()`。`changes.rs`（R-04）：`parse_hunk_header` / `diff_hunk_line_numbers` 从 `@@ -a,b +c,d @@` 本地递推双列行号（context 左右 +1、deletion 只左、addition 只右、畸形头空行号不伪造，不改 wire），diff 整行浅底 token `diff.addition_bg=#1b2a22` / `diff.deletion_bg=#2a1c1e` / `diff.gutter_fg=#9b9ba4`（不再复用 Allow / Deny 按钮色），行号 gutter 32px × 2；文件清单改 32px 紧凑行：`M` 琥珀 / `A` 绿 / `D` 红色标（其余状态原样）+ 路径 + 右对齐 `+A/−D`，`changes-file-*` identifier 与选中 / hover / 焦点语义不变；只读、latest-session mismatch banner、断线 stale、空面板不自动打开不变。`theme.rs` 新增 `INSPECTOR_BACK_HEIGHT=36` / `CHANGES_FILE_ROW_HEIGHT=32` / `DIFF_LINE_NUMBER_WIDTH=32`；`components/panel.rs` 增 `Panel::fill()`。
- **自动检查**：Desktop 定向测试 243/243（新增 `resolve_inspector_placement_follows_width_and_preference`（1440 Side / 1080 Center / 150%×1280 Center / 未打开 Hidden / 变宽变窄）、`hunk_line_numbers_advance_from_header_and_fail_closed`、`work_panel_center_mode_reaches_all_tabs_and_restores_conversation`（1080×720 三面板中央可达、`inspector-back` 返回后草稿与 Timeline 偏移保留、中央模式无 Timeline / Composer 节点、无 terminal / cancel 派发）、`inspector_panel_selector_keyboard_and_approval_hint`；更新 `resolve_switches_rail_width_at_1280`、Inspector 键盘 / 焦点、theme 常量、i18n、AX 裁剪测试）。日志 `/tmp/pawork-gui2-05-tests.log`。
- **限制**：未做真窗口像素复验（三面板最小窗口 / 150% 可达、开合不重建终端为 AX + 命令派发断言）；`inspector-tab-*` 未改名。代理真窗口 2026-09-14 随 GUI2-07 批次通过（变更面板与双列行号 diff、1080 最窄窗中央面板与「返回对话」恢复、终端 PTY echo 真实往返、资源面板空态、面板 150% 窄窗，证据 06～10 / 13～15 / 22）；用户验收未进行。

### GUI2-06 设置查找与信息组织

- **当前基础**：已有八页设置、全宽滚动、自然增高供应商卡、已连接优先、多账号 / 三窗额度和模型目录搜索。
- **改动**：按 [设置规格](gui-design.md#9-settings) 增加设置名称查找，命中后进入真实页面并定位对应行；与 GUI2-03 共享同一份页面 / 设置标题条目。保留八页与能力 gate，以标签、说明、当前值、动作四层整理密集设置，账号及额度详情按需展开。
- **写入集**：`ui/settings/` 中受影响页面、查找模块、`ui/mod.rs`、i18n 与对应 Settings AX；不改认证、quota 与配置后端。
- **验收**：搜索「字号 / 模型 / 代理 / 审批」能进入可用行；不可用页不伪造结果，定位不自动保存或切换设置。默认角色仍位于供应商后，识图 / 搜索保留尚未生效说明；账号选择只影响后续请求，过期与未知额度不显示为 0。返回工作台保留草稿与 Run。
- **最小切片**：先搜索与定位，再按实际拥挤点整理 Providers / 普通设置行；不重做八页数据流。
- **显示效果补充（2026-09-12 Review，见 §4 R-08）**：供应商卡头由 `px_4 py_4` 两列文字收为 48–56px 单行（状态色点 / 名称 / 认证方式 / 连接 chip / chevron），当前卡头名称与「已连接 · N 个模型」之间留白过大；八页导航按「模型 / 工作台 / 系统」分组并在 GUI3-05 图标体系落地后加图标；Approvals 五档 `●` / `○` 字形改真实圆环 radio；Switch 关态 hover 不再走 `accent.hover`。验收补：卡头高度与导航选中零位移（F4）继续由既有 Settings AX 钉住。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：新增 `ui/settings/search.rs`——静态目录 `settings_search_entries()`（每条 `page` / `row_id` / 中英标题 / 说明 / 别名，覆盖字号、语言、模型 / 默认模型 / 管理模型、供应商 / API key / OAuth、代理、审批模式 / 会话信任、MCP、终端 shell / 尺寸、连接诊断、数据目录）与 `filter_settings_search_entries`（忽略英文大小写，按页面 gate 过滤：Providers 常在、Host 页看 `query.available`、About 需连接 + `host_data_dir`、Appearance / Advanced 离线可用）。Settings 左栏返回按钮下增独立 `TextInput`「查找设置 / Search settings」（`settings-search-input`，不进 Composer 草稿、不读 secure 缓冲，离开 Settings `reset_settings_search` 清空）；查询非空时导航被结果列表替换（`settings-search-result-{Page}-{row_id}` ListItem 可 Press，空结果 `settings-search-empty`），↑/↓ 高亮、Enter 选择、Esc 清空回焦。选择命中 `locate_settings_entry`：先切页（滚动归零），下一帧 `finish_settings_locate` 用 `settings_element_layouts` + 页面 ScrollHandle 滚入目标行，约 2s 以 `surface.hover` + 2px accent 左缘（绝对定位，不进盒模型）标识并把焦点落到对应控件 / 只读行；只导航，不派发任何写命令。`quick_search.rs` 删除 `SEARCH_PAGES`，「页面」分组改从 `settings_search_entries()` 派生（空查询只出八页标题，非空查询加入设置行 `SearchTarget::SettingsRow`），断线 gate 仍只允许 Appearance / Advanced。R-08：`providers.rs` 供应商卡头收为固定 52px 单行五列（中性状态色点绿 / 灰 / 红 · 名称 · 认证方式 · 「已连接 · N 个模型」chip · chevron；`settings-provider-header-{id}` 布局 id 钉 48–56px，既有 name / connection / catalog / expand identifier 不漂）；八页导航按「模型（Models & providers）/ 工作台（Network、Approvals、Tools & MCP、Terminal）/ 系统（Appearance、Advanced、About）」分组加 12px 组头（`settings-nav-group-{models,workspace,system}`，组内全隐则组头隐；F4 零位移仍成立）；`permissions.rs` 五档 `●` / `○` 改真实圆环 radio（Ø16 外环 1.5px `text.secondary`，选中 Ø8 accent 实心，`settings-approval-mode-{wire}` 与 Press gate 不变）；`components/switch.rs` 关态 hover 改 `surface.hover`，滑块仍业务态驱动无动效；`appearance.rs` 字号 / 语言改标签 + 当前值纵排。未改认证 / quota / 配置后端与 Host 命令语义。
- **自动检查**：Desktop 定向测试 246/246（新增 `settings_search_entries_cover_required_topics_aliases_and_case`、`settings_search_entries_filter_by_page_gate`、`settings_search_locates_rows_without_writes`（中英搜索「字号 / 模型 / 代理 / 审批」进入正确页、目标行滚入且 AX 存在、无写命令派发、不可用页无结果、断线只剩本地页、Esc 清空回焦、返回工作台草稿保留）；更新 `settings_nav_ax_frames_stay_put_across_selection_change`（含分组头）、`settings_controls_follow_layout_and_scroll`、`settings_ax_masks_api_key_and_gates_writes_when_stale`（卡头高度）；`quick_find_*` 两条继续通过）。日志 `/tmp/pawork-gui2-06-tests.log`。
- **限制 / 偏离**：Network 的输入 | Save | Clear 横排与 Terminal 等页未做全面四层重排（只整理 Appearance 与供应商卡头）；查询激活时导航整体被结果替换；状态色点为装饰不进 AX；导航图标待 GUI3-05、Switch 动效待 GUI3-08。代理真窗口 2026-09-14 随 GUI2-07 批次通过（设置审批页定位与「信任项目」、外观 150% 即切、Cmd+K 命中「审批模式」设置行，证据 04 / 20 / 24）；用户验收未进行。

### GUI2-07 主路径验收与记录收口

- **任务**：用最终候选复验本阶段改变的路径，并补齐 §3 中与这些路径相关的旧证据；只修实际复现的问题。
- **场景**：首页 → 真实项目新任务 → 输入 / 模型 → 流式与工具 → 审批 → Changes → 窄窗面板 → 返回继续；另外覆盖快捷查找、长对话定位、设置查找、断线恢复。真实模型固定当次 `opencode-go / glm-5.3-flash`。若 §4 的 GUI3-01～04 先于本项完成，同批复验工具 headline、Composer 元信息、代码块头与失败卡在真实 Provider 回合中的呈现。
- **证据**：界面像素 / 键盘操作与 Host、文件、Git 或 PTY 结果对应；fixture 只用于确定性布局与状态覆盖，不冒充真实 Provider、订阅额度或用户签字。
- **收口**：逐项更新实现、自动检查、代理真窗口与用户验收状态；被认证 / 账号 / 捕捉能力阻断的项写清实际终态，不整体宣称通过。只有用户验收完成的项才登记已归档。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选，部分进行）**：候选二进制 `scripts/pawork-desktop.sh build` 编译通过（`pawork` + `pawork-desktop`，debug；desktop 26 条 dead-code / unused-import 警告，含本轮新增的测试专用函数与保留常量，未阻断）。隔离实例 `--instance gui2-07` 起 Host（`--provider opencode-go --model glm-5.3-flash --approval-mode ask-for-writes gui serve`，不写持久默认）：
- `pawork-desktop --probe`：`connected: sessions=0 models=48`，目录含 `opencode-go/glm-5.3-flash`。
- `pawork-desktop --probe-smoke`：exit 0，`first=glm-4.7 first_turn=pong second=deepseek-v4-flash second_turn=pong assistant_turns=2 cancelled=1 persisted=14 disconnect_survive=running`。注意 probe-smoke 的模型对是 `main.rs` 里**既有硬编码**的 `glm-coding/glm-4.7` → `deepseek/deepseek-v4-flash`（真实 Provider 回合成功），并非本次指定的 `opencode-go / glm-5.3-flash`；写文件回合 `approval=not_requested`，仓库无残留文件。日志 `/tmp/pawork-gui2-07-{build,host,probe-smoke}.log`。
- 真窗口：平行 bundle `/tmp/Pawork-gui2-07.app` 三次启动（`open -na` × 2、直接执行 × 1）均出窗（CGWindowList `onscreen alpha=1.0`，1440×1024），`lsof` 证实 Desktop 进程持有到 `pawork-gui-gui2-07.sock` 的已接受连接，stderr 无 AX 安装失败。**但取证被阻断**：`screencapture -l/-R` 返回 `could not create image`（本代理 shell 无屏幕录制权限）；`scripts/ui-ax-dump.swift` 对 Pawork 进程持续得到 AXApplication 自递归、`kAXWindowsAttribute` 为空、0 个业务 identifier（同工具对 Finder 正常，`ui/accessibility/macos.rs` 相对 HEAD 无 diff；符合脚本注释登记的 macOS 26 间歇签名，但本轮三次均复现，需人工开窗核实是否与本工作区改动相关）；`ui-focus-switch.sh activate` 未收敛。因此 §GUI2-07 场景（真项目新任务 → 流式 / 工具 → 审批 → Changes → 窄窗面板 → 返回；快捷查找 / 长对话定位 / 设置查找 / 断线恢复）与 GUI3 各项的真实 Provider 回合呈现**均未取得像素或 AX 证据**，登记为未进行；未修改任何代码。副产物：`~/.pawork/gui2-07/`（隔离实例数据）保留待用户处置。

**本轮进展（2026-09-14，实例 vfix，取证恢复）**：2026-09-12 的取证阻断今日未复现——computer-use AX 树读取与 `screencapture -l` 均正常。载体：Host `pawork --instance vfix --provider opencode-go --model glm-5.3-flash gui serve`（10:4x 起 `--approval-mode ask-for-writes`，11:06 重启后为 `ask-for-dangerous`）+ 平行 bundle `target/pawork-desktop-runtime/Pawork-vfix.app`（当前工作区增量构建，含 V 批修复与 D1 逐事件时间戳）。真窗口 1087×727（1080 为最窄支持宽）。证据 `/tmp/pawork-gui2-07-evidence/`（01～25，编号 17 跳过），未检入仓库。

- **首页 → 真实项目新任务（01）**：新建绑定 `pawork-gui2-07-ws` 的任务（session.db：`ses-1789353974010-1` / `ws-1789353973957-1`），空任务「开始对话」引导、工作区 chip、`glm-5.3-flash` 模型触发器渲染正常。
- **流式与工具（02 / 03）**：首轮 `read_file` 成功、`write_file` / `apply_patch` 被「untrusted workspace」策略诚实拒绝，工具组 headline 折叠（`2 个工具 · read_file README.md · write_file NOTES.md · 1 已完成 · 1 失败`）与展开失败原因原文均正确；助手终态如实说明未写入。
- **审批（04 / 05）**：经设置 → 审批页「信任项目」（04）后第二轮写入触发审批卡（`write_file · NOTES.md`，允许一次 / 允许本次运行 / 拒绝），放行后写入成功。session.db 核对：`tool_approval_requested` / `tool_approval_responded` 事件对与 `checkpoint_created` 齐全。
- **Changes（06 / 07）**：变更面板列出 `A NOTES.md +4 -0`，diff 双列行号与整行浅底正常；磁盘 `/tmp/pawork-gui2-07-ws/NOTES.md` 内容与 diff 逐行一致；「待审阅」卡与「查看变更」入口在。
- **窄窗面板与返回（08 / 09 / 10）**：1080 最窄窗下面板中央呈现、顶部「返回对话」返回后对话与 Composer 恢复（08 与 09 同帧：窗口已处最窄，resize 未产生新几何）。
- **长对话定位（11）**：回合定位至首轮，内容溢出时「回到底部」按钮出现。
- **断线恢复（12 → 13 / 14）**：Host 重启窗口期主区「未连接」+「重试连接 / 连接诊断」、Composer 禁发、草稿与历史保留、侧栏底部「本地 · 已断开 · 重试」；重连后终端面板 PTY 真实往返（`echo gui2-07-pty-ok` 回显）。
- **快捷查找 × 设置查找（24）**：`Cmd+K` 浮层查询「审批」命中设置页与「审批模式」设置行（GUI2-06 条目共享），类型图标与 accent 高亮条渲染。
- **取消（18 / 19）**：300 行长输出 run 完成后新起 run，1 秒后取消——页脚「运行已取消 · ↑ — · ↓ — · 时长 00:01」（终态无持久化 usage 按未知显示），DB 状态 `cancelled`。
- **150% 字号（20 / 21 / 22）**：外观设置切 150% 立即生效；最窄窗 + 150% 下工作台与面板无控件遮挡（底栏状态簇偏挤但可读）。
- **失败卡（23）**：`run-gui-1789353068886-7` 因 Host 中途终止落 `run_failed`（"host process ended before the run reached a terminal state"），紧凑 banner（Ø20 状态圆 + 单行标题 + 原因原文），非认证失败正确无 CTA；页脚时长 11:52 为真实跨度。
- **真实回合表格（25）**：表格表头 `surface.hover` 底色、列宽按内容实测无 160px 空洞；该 run 终态 `↑ 2996 · ↓ 58 · 时长 00:02 · 27 tok/s` 与 DB `run_completed` payload 逐值一致。

**D1 实证**：今日四轮 run 的事件 distinct timestamp_ms 分别为 101/105、31/37、130/133、12/13（事件总数为分母），终态时长（00:07 / 00:45 / 00:17）与 tok/s（35 / 98 / 27）在持久化数据与 UI 两侧同时成立；usage 对账 `run_json` ↔ 页脚逐值一致（3169/87、2996/58）。

**其他观察（登记，不在本批修复）**：

- Host 日志出现 `usage record id conflict: rec-{run_id}` warn：同一 run 的用量账本被二次写入并被幂等冲突守卫拒绝。账本首写成功且数值与 UI 一致（`usage-ledger.sqlite3` 六行逐值核对），无错账；但二次写入的来源未定——`record_completed_usage` 唯一调用点在 run 收尾路径，而 `occurred_at_ms` 取记录时刻墙钟使任意重入必冲突。`run.rs` / `usage.rs` 不在本批写入集，属既有行为；建议另立任务仪表化定位重入点（疑似与 Host 重启后对历史 run 的某种重放 / 重扫相关，冲突行在 10:4x 与 11:06 两次 Host 启动后均出现）。
- ADR-061 Go 三窗 >0% 填充仍无像素证据（2026-09-14 下午再核：多次真实 run 后上游读数仍为 0%，属环境不可得）；GUI3-04 的 401 CTA 已于 2026-09-14 下午补齐（见 §2.2 末尾补录）。GUI2-01 / 02 轮的 401 属旧失败卡设计，不作本项证据。
- GUI3-08 动效（Switch 位移、骨架屏）静态截图无法取证，留用户验收。
- 底栏常驻「需要快照 · 从 0 开始」（`resume.snapshot_required`）为既有 resume 功能文案，非本轮回归。

**收口状态**：GUI2-07 代理真窗口通过；GUI2-05 / 06 与 GUI3-01～03 / 05～07 的代理真窗口随本批完成；用户验收仍未做，未归档。

### 2.1 用户视觉走查发现（2026-09-13）

本机 `desktop` 实例 + 平行 bundle `Pawork-visual.app`（当前工作区增量构建）。下列为用户指出后交叉核过的疑似问题。后续观感项按编号续记。

| 编号 | 现象 | 核对 | 判断 | 状态 |
| --- | --- | --- | --- | --- |
| V-01 | Settings / 模型菜单的 OpenCode Go 看不到 `deepseek-flash` | 见下 | 远端已返回该 ID；旧实现按 transport 表当白名单丢弃未登记项 | 已修（2026-09-13）：表与家族只路由，新聊天 ID 不再丢弃 |
| V-02 | Settings 启用或禁用单个模型时界面卡一下 | 见下 | 每次 Switch 都在 Host 重跑全通道 `models_overview`，回执后再拉两套 `model_list` + `provider_auth_status` | 已修（2026-09-13）：启停用目录快照校验，回执本地收敛，不再重探 |
| V-03 | Composer 模型菜单每个模型都带一段供应商介绍 | 截图 | 扁平分组把连接/目录来源重复铺在组头、状态行和每行 `provider / id` | 已改（2026-09-13）：同表伪二级（组头 + 模型），隐藏未连接 / 0 启用 |
| V-04 | 任务行悬停改名 / 归档与项目头计数 / 「+」纵向错位 | 走查 | 项目头 56+36 尾槽、任务动作 32+32 且 `right(8)` | 已修：共用 64px / 8px inset 尾槽 |
| V-05 | 未选任务不能直接开对话 | 走查 | `can_send` 要求 `active_session_id` | 已修：无任务发送走 Unassigned |
| V-06 | 发送后 Cancel 变红但无 icon | 走查 | `cancel.svg` 为 stroke-only，gpui mask 看不见 | 已修：实心停止方块 |
| V-07 | 对话中看不到 token 数 / tok/s | 走查 | 用量只在「···」菜单；live `RunChanged` 无 usage | 已修：状态栏与页脚展示权威用量；有时长才给 tok/s |
| V-08 | 对话排列仍明显疏于 Codex | 走查 | 32px 回合距、助手常驻「Pawork」、用户气泡偏淡 | 已改：16px 间距、去常驻作者行与空闲作者槽、气泡 / 工具组层次 |
| V-09 | 思考与正文、正文与页脚留白过大 | 截图 | 助手空闲仍占 24+12 作者行；思考头 36px；回合距 24 | 已修：空闲助手不占作者行；思考头 24px；距 16/12/8 |
| V-10 | 页脚同一行不对齐；模型选择器箭头离名称太远 | 截图 | 页脚 `flex_wrap` + 时间与菜单分行；触发器固定 220px 且 `flex_1` | 已修：用量 / 时间 / 菜单单行；触发器随名称收缩 |
| V-11 | 无滚动条仍显示「回到底部」 | 走查 | 展开思考等会 `timeline_following=false`，短对话也画按钮 | 已修：仅内容溢出时绘制 |

**V-01 细节**：不是「没查到最新模型」。

- 公开 `GET https://opencode.ai/zen/go/v1/models`（2026-09-12 23:11+08，HTTP 200）返回 **37** 个 ID，其中含 `deepseek-flash` 与 `deepseek-v4.1-flash`。官方 [Go 文档](https://opencode.ai/docs/go/) 已把 **DeepSeek V4.1 Flash** 登记为 `deepseek-v4.1-flash`，协议 `POST …/chat/completions`；`deepseek-flash` 出现在公开 `/models`，文档 endpoint 表未单列该短 ID。
- 修复前同机 `pawork --instance desktop --json models`：`opencode-go` **20** 项，没有 `deepseek-flash` / `deepseek-v4.1-flash`。stderr 只有 xAI 探测失败，Go 远端替换已成功。
- 旧机制：`list_models` 对未登记 ID 返回 `None` 并 `retain` 丢掉。2026-09-13 起官方表不再当白名单：未登记 ID 按家族回退或 Chat Completions 进入可运行目录；仅 Messages-only（`qwen*` / `minimax-*`）与 Qwen 非文本仍排除。ADR-058 D2 已同批修订。
- 对照：直连 `deepseek` 通道当次目录已有 `deepseek-flash`，与 Go 通道不是同一条；未把该 ID 的真实 completion 记为已验收。

**V-02 细节**：不是 Switch 动画或 AX 树本身卡死。

- 修复前每次单模型启停：Host `set_model_enabled` 先跑全通道 `models_overview`（各通道 4s 探测上限），写盘后再由 Desktop `refresh_models_authority` 拉 `provider_auth_status` + 两套 `model_list`，等于同一动作最多四轮全网探测。
- 2026-09-13 起：启停校验优先最近一次 overview 快照 / 静态回退 / 已在 denylist 的 ID；回执本地收敛弹层、Composer 过滤目录与 `cleared_roles`。打开 Manage models、页级 Refresh、认证成功/移除仍重查。未把真窗口手感记为已验收。

**V-03 细节**：不是再点一层供应商。Composer 菜单与 Settings 角色菜单同一形态：已连接且有启用模型的供应商作不可点组头，模型（名称 + ID）列在下面；未连接、清单缺失或 0 启用整组不出现。未把真窗口手感记为已验收。

### 2.2 代理真窗口验收（2026-09-14，实例 vfix）

载体：Host `pawork --instance vfix --provider opencode-go --model glm-5.3-flash --approval-mode ask-for-writes gui serve` + 平行 bundle `target/pawork-desktop-runtime/Pawork-vfix.app`（工作区增量构建，含本节末尾 V-12 修复）。取证经 macOS 无障碍树 + 真窗口截图（computer-use 驱动）。定向门禁：`cargo test -p pawork-providers -p pawork-auth -p pawork-protocol -p pawork-app -p pawork-desktop --offline --lib --tests` 全绿（desktop 单跑用 `--tests`，260 通过 0 失败）；独立审查代理逐 hunk 复核全量 diff 无 P0/P1（P2 见文末）。

| 项 | 结果 | 证据 |
| --- | --- | --- |
| V-01 | 通过 | Manage models 弹层：`已连接 · 远程目录 · 已启用 3 / 28`，含 deepseek-flash（开）、deepseek-v4.1-flash（关）等未登记 ID；命名角色默认已绑 `OpenCode Go · deepseek-flash` |
| V-02 | 通过 | 开关 deepseek-v4-flash：回执本地收敛 `已启用 3/28 → 4/28`、供应商行 `3 个模型 → 4 个模型`，全程约 1.5s（含 AX 抓取），Host 日志零新增探测行（启动期 xai / opencode-go 探测超时 warn 对照仍在原处） |
| ADR-061 重命名 | 通过 | 行内 Enter 提交 `swkee86 → vfix-go-main`，回执 + AuthChanged 后投影更新（重开编辑器预填新名），空名 / 同名不提交走既有 decision 分支 |
| ADR-061 默认名与脱敏 | 通过 | 卡头无手填名要求；副标题 `sk-…xpZn · 已连接`；源码核实 `default_api_key_account_name` 只产出脱敏档（`••••` / `xx***yy` / `xxxx***yyyy`），不可能输出原文片段 |
| ADR-061 Go 三窗额度 | 部分通过 | 三窗各行 `已用 0% · 剩余 100%` + 重置倒计时 + 来源/获取时刻真实（`opencode-go/usage`）；快照 TTL 过期后标 `已过期 · 请刷新`，刷新额度后恢复；进度条轨道可见但 0% 填充，>0% 填充未取到像素 |
| V-03 | 通过（08:2x 补验） | 菜单伪二级像素确认：OpenCode Go / GLM Coding / Kimi Code 不可点组头 + 缩进模型行（选中行 ✓ 高亮）；未连接的直连供应商整组不出现；Kimi Code 启用 1 个（`k3`，搜索 kimi 可见组内行）；证据 `/tmp/pawork-vfix-evidence/vfix-modelmenu-crop.png`、`vfix-mm3-crop.png` |
| V-04 | 通过 | 绑定 Pawork 项目后项目视图：项目头计数 `1` + 「+」与任务行 ✎/🗑 共用 64px / 8px 尾槽，4 倍放大右缘逐像素对齐（`vfix-rail-zoom4x.png`） |
| V-05 | 通过 | 未选任务直接发送即建 Unassigned 会话（`ses-1789344458955-1`）并开 Run；侧栏出现「未分组」组 |
| V-06 | 通过 | 运行中 Composer 右下红色圆形 Cancel + 实心白色停止方块（`vfix-cancel-zoom.png` 原像素）；输入区提示「任务运行中——发送已禁用，仍可取消」 |
| V-07 | 通过（一处观察见「其他观察」） | 四轮真实 Run（opencode-go / glm-5.3-flash）全部成功流式；页脚 / 底栏同源 `↑ 5536 · ↓ 6879` 与 session.db `run_completed` 持久化 usage 逐值一致；live 行 `↑ 84 · ↓ 1497 · 00:07` 随流式跳动（`vfix-r4-3.png`） |
| V-08 | 通过 | 无常驻作者行 / 空闲作者槽；气泡 → 思考 → 正文层次紧凑，回合距观感与 Codex 相当（`vfix-turn-gap2x.png`） |
| V-09 | 通过 | 思考头收敛态约 24px；展开后思考内容 → 正文标题间距紧凑（`vfix-think.png`）；glm-5.3-flash 本轮产出真实 thinking 段 |
| V-10 | 通过 | 页脚「运行已完成 · ↑ · ↓ · 时长 · 相对时间 · ···」单行不换行；Composer 模型触发器随名称收缩、箭头紧邻 `glm-5.3-flash` |
| V-11 | 通过 | 空 / 短对话无按钮；长对话贴底跟随中无按钮；上滚越过末行后出现「↓ 回到底部」，点击回底且按钮消失（`vfix-btb.png`） |
| V-12 | 复验通过 | 卡头 `vfix-go-main`（重命名经 Host / Desktop 重启后仍在，持久化确认）与三窗标签 `5 小时窗口 / 周窗口 / 月窗口` 均像素渲染（`vfix-v12.png`） |

**V-12（本轮新发现，已修复并复验通过）**：Settings 账号卡头名称与 Go 额度行窗口标签（`5 小时窗口` 等）在真窗口不渲染——截图中卡头仅有状态点与右侧操作簇、额度行仅有右侧百分比，而 AX 摘要（手写树）含名称，属「AX 正常、像素缺失」。定位：两处新增文本用 `flex_row > flex_1+min_w_0 包裹 > overflow_hidden > 可换行 Label`，同一构造在 flex_col 父级下（掩码副标题行）正常，在 flex_row 行内（有固有宽度兄弟）被内容测量趟归零；修复改为 timeline_entry 已验证的 `truncate()` 单行构造（同层 flex_row + flex_1 + min_w_0，去掉 overflow_hidden 包裹与换行 Label）。定向测试 260 绿；2026-09-14 08:3x 像素复验通过（`vfix-v12.png`：卡头 `vfix-go-main` 与三窗标签齐全）。

**阻断（已解除）**：2026-09-14 01:20～08:0x 本机锁屏，computer-use 无法读写 UI（自动解锁失败）。08:0x 用户解锁后重启 Host 与 `Pawork-vfix.app`（修复构建原样保留），V-03～V-11 像素、V-12 复验与四轮真实 Run 全部补验完成，结果见上表；证据截图归档 `/tmp/pawork-vfix-evidence/`（不写仓库）。

**其他观察**：

- 冷启动首次加载提供商状态 / 目录超过 GUI 客户端 10s 接收上限（xai、opencode-go 冷探测超时叠加），横幅报 `operation receive frame timed out after 10s` 并保留 stale 列表；数据后续到位，页级刷新后即正常。既有行为口径（打开弹层 / 刷新仍重探），非本批回归。
- 合成事件下改名 Enter 提交后编辑器复开（第二路 Enter 落到回焦的「重命名」按钮再进入编辑，预填已是新名，即提交本身成功）；与代码内登记的 AppKit Return 双路投递同一形态，真人路径是否复现留人工验收确认。
- 审查 P2 处置（2026-09-14 收口，逐项经独立决策评审）：① Go 通道非文本误纳——已修：`inferred_transport` 的 opencode-go 分支先过共用 `non_text_model` 谓词再走家族回退（与 qwen 通道同规则），`go_keeps_undeclared_ids_and_routes_by_family` 补 `gpt-image-1 → None` 断言；当前远端 37 个 ID 全为文本模型，属防御而非现行 bug。② 终态无持久化 usage 的无标记估算——已修：`run_usage_display` 回落分支按 live 分叉，终态无 usage 显示 `↑ — · ↓ —`（gui-design「缺失为未知」），字符估算只服务 live 预览；新增 `run_status_label_terminal_without_persisted_usage_shows_unknown` 回归。③ `pending_home_send`——不修：create 四条路径均经进程内 channel 发 SessionCreated / OperationFailed 回执（各 10s 超时），「永久锁死」前提不成立；断线期间清 pending 反而会丢用户消息。④ 共享 Button 去 `min_w_0`——代码不动，已补真窗口目检：turn 标题由投影层预截 72 字符，在最窄支持窗宽 1080px（`WINDOW_MIN_SIZE`，640px 不可达）下长标题在面板内单行截断、无溢出 / 换行 / 重叠（`/tmp/pawork-d5-turns.png`）；task_rail 定宽调用点无可观测差异。
- 终态 run 的「时长」恒 `—`——已修（2026-09-14，待用户确认口径）：engine `EventEmitter.emit` 改为逐事件取当前墙钟（删构造期钉入的 `timestamp` 字段，三处调用点去参；`SessionTurn.timestamp` 保留）。此前同 run 全部事件共享同一毫秒（session.db 实测 distinct=1），`run_span_ms` 恒 None，终态时长与 tok/s 在持久化数据层面不可能成立。持久化按 sequence 排序落库、重放按 sequence 游标，均不依赖同戳语义。定向门禁 `cargo test -p pawork-providers -p pawork-engine -p pawork-desktop --offline --lib --tests` 全绿（261 / 66 / 156）。若用户否决该语义改动，回退 `crates/engine/src/event.rs` 与三处调用点即可。

**非空 MCP 补验（2026-09-14 下午，实例 vfix，已完成）**：fixture 为本地 stdio echo 服务器（/tmp/pawork-mcp-ws/mcp_echo.py，单工具 `echo`，已补 readOnlyHint）+ 工作区级 `/tmp/pawork-mcp-ws/.pawork/config.toml` 的 `[mcp.servers.echo]`（fixture 资产，保留在 /tmp）。已取证：① 真窗口 Settings → 工具与 MCP 行 `echo — connected · stdio · 1 个工具`（像素 /tmp/pawork-mcp-evidence/01-mcp-tools-page.png，AX `settings-mcp-server-echo` 同源）；② CLI `mcp list` → `echo stdio connected echo.echo`；③ headless 真实 run（opencode-go / glm-5.3-flash）中模型真实调用 `echo.echo`：session.db 有 tool_approval_requested/responded 与 run_completed 全链（usage input 10460），首轮因工具无 readOnlyHint 在 never-ask 下被自动拒绝（`tool call denied by user`），补 readOnlyHint 后④ **GUI 内成功执行**：绑定 pawork-mcp-ws 的会话发送调用请求，ask-for-dangerous 下只读工具免审批直接执行，时间线为工具组 `echo.echo` + 助手终文 `pawork-mcp-gui-ok` + 页脚 `运行已完成 · ↑ 2474 · ↓ 29 · 时长 00:03 · 7.7 tok/s`（像素 /tmp/pawork-mcp-evidence/02-mcp-gui-success.png）；session.db 核对 seq 8→12 `tool_call_arguments_delta → tool_execution_started → tool_execution_completed`（`tool_name=echo.echo`、`is_error=false`、返回 `echo: pawork-mcp-gui-ok`）→ `run_completed`，usage 2474/29 与页脚逐值一致，事件 timestamp_ms 两两不同（D1 又一实证）。

**GUI3-04 401 CTA 补取（2026-09-14 下午，同批）**：给 deepseek 临时写入无效 key（CLI `auth set-key`，GUI 验证流程会拦截无效 key 故走 CLI），Composer 切 DeepSeek Chat 发送 → 失败卡 Ø20 红圆 +「运行失败」+ 原文 `HTTP 401` + CTA「打开供应商设置」（像素 /tmp/pawork-mcp-evidence/04-gui3-04-401-cta.png）；点 CTA 导航到设置 → 模型与提供商（05-gui3-04-401-cta-target.png）。取证后已删除无效 key 并重启 Host 恢复现场。**操作事故登记**：set-key 前误信凭证按实例隔离，备份了实例 protected 库；实际 Provider 凭证在全局 `~/.pawork/auth.json`（`pawork.<provider>` 条目），备份不含它——deepseek 真实 key 被无效值覆盖且本机无快照可恢复，已删除无效条目（fail-closed，现状为未配置），**需要用户在设置 → DeepSeek 重新录入真实 API key**。kimi-platform 的临时无效 key 也已删除（原状态即未配置）。教训已转化为本次操作流程：改全局凭证前先备份 `~/.pawork/auth.json` 本体。

**ADR-061 三窗 >0% 填充（2026-09-14 下午再核）**：opencode-go 账号 5 小时 / 周 / 月窗口经多次真实 run 后仍 `已用 0% · 剩余 100%`（设置页「刷新额度」实时拉取，3 秒前获取），>0% 填充属上游读数环境不可得，非代码缺陷，继续留待账号产生真实用量后补取。2026-09-14 18:2x 第三次复核（直连 `GET /zen/go/v1/usage`，UA `pawork`，HTTP 200）：rolling / weekly / monthly `percent` 仍为 0（resetsAt 13:14Z / 09-21 / 10-10），环境不可得结论不变。

**其他登记**：① `usage record id conflict`（已修复，2026-09-14 晚）：warn 在 16:24 Host 重启后再现（`rec-run-gui-1789344458971-1`）。根因：`services/run.rs` 的 request_id 为 `req-{n}`（AppCore 进程内计数器从 1 起），Host 重启归零后与历史 run 撞键；账本按 (tenant, account, request_id, upstream_attempt) 去重（冻结契约），把新 run 的合法记录当成同一请求的重放拒收——不只是 warn，16:14 echo run（2474/29）与之后多轮用量实际未入帐，账本自 11:06 重启起持续漏记。修复：request_id 改 `req-<pid_hex>-<nanos_hex>-<n>`（与 client `new_request_namespace` 同形态；毫秒粒度不够，测试实证同毫秒双 AppCore 仍撞），回归 `run_request_id_survives_counter_reset_across_host_restarts`。验证：`cargo test -p pawork-app --offline --lib --tests` 全绿（223+6+16+2）；修复后新 run 的 `req-d775-…-1` 记录成功入帐。既有漏记行不回填（账本 append-only，值以 session.db 为准）。② **新发现缺陷（已修复，2026-09-14 傍晚）**：`pawork run` 在实例已存在 open session 时复用该 session 并返回首个 run 的 run_id，随后不产生任何事件、无超时挂起（三次复现）。根因不是 InFlight / 事件泵：CLI 的 command_id 为 `cli-cli-json-<n>`（进程内 AtomicU64 从 1 起），command_ledger 以 (tenant, automation, command_id) 持久幂等，跨进程必撞键——第二次 `pawork run` 的 SessionCreate / RunStart 直接重放首次响应（旧 session、已完成 run），随后等待永不到达的新事件。修复：`crates/cli/src/adapter.rs` 的 command id 加进程级命名空间（pid + 纳秒 OnceLock，与 client `new_request_namespace` 同形态），回归 `command_ids_carry_process_namespace_and_stay_unique`；同 id 重试的进程内幂等不受影响。验证：`cargo test -p pawork-cli --offline --lib --tests` 全绿；同实例复跑 `pawork run --json` 新建会话与新 run，41 行事件流至 `run_changed completed` 自行退出。

## 3. 前阶段验收承接

下表来自迁存的原批次记录，不是本次重新执行的结果。UX-01～09 均已实现并记录定向检查通过，用户验收与任务归档均未完成。旧问题不重新开发；复现回归才在对应 GUI2 任务内修复。

| 原任务与证据入口 | 原批次代理真窗口状态 | 仍需补齐 / 承接 |
| --- | --- | --- |
| <a id="ux-01-长草稿自然换行与编辑"></a>[UX-01 输入](review/roadmap-ux-2026-09-10.md#ux-01-长草稿自然换行与编辑) | 部分通过 | 换行实现后的系统 IME；随 GUI2-02 |
| <a id="ux-02-回复可阅读可复制可使用"></a>[UX-02 阅读](review/roadmap-ux-2026-09-10.md#ux-02-回复可阅读可复制可使用) | 部分通过 | 用户验收（2026-09-14 GUI2-07 批次已成功流式至多轮完成、重开与重连恢复一致；原 401 缺口闭合） |
| <a id="ux-03-新任务与项目上下文引导"></a>[UX-03 项目引导](review/roadmap-ux-2026-09-10.md#ux-03-新任务与项目上下文引导) | 通过 | 用户验收（壳层改动后复验已由 2026-09-14 GUI2-07 批次覆盖） |
| <a id="ux-04-错误在发生处解释并提供下一步"></a>[UX-04 恢复](review/roadmap-ux-2026-09-10.md#ux-04-错误在发生处解释并提供下一步) | 通过 | 用户验收（连接 / 面板改动后复验已由 2026-09-14 GUI2-07 批次覆盖：断线 / 重试 / 重连恢复 + 三面板） |
| <a id="ux-05-模型选择与管理效率"></a>[UX-05 模型查找](review/roadmap-ux-2026-09-10.md#ux-05-模型选择与管理效率) | 部分通过 | 备用目录鼠标状态、管理搜索 / Switch 键盘及完整宽窄字号矩阵；随 GUI2-02 / 06 |
| <a id="ux-06-供应商与账号信息层级"></a>[UX-06 账号与额度](review/roadmap-ux-2026-09-10.md#ux-06-供应商与账号信息层级) | 部分通过 | 真实数值 / 倒计时 / 过期读数、双账号切换；随 GUI2-06，等待可用账号 |
| <a id="ux-07-工具与运行事实可核查"></a>[UX-07 执行事实](review/roadmap-ux-2026-09-10.md#ux-07-工具与运行事实可核查) | 部分通过 | 用户验收（2026-09-14 GUI2-07 批次已覆盖成功工具、非零 usage 对账、取消与后续新 run 一致） |
| <a id="ux-08-归档的可恢复性"></a>[UX-08 撤销归档](review/roadmap-ux-2026-09-10.md#ux-08-归档的可恢复性) | 通过 | 用户验收；侧栏改动后定向复验 |
| <a id="ux-09-视觉文案与可访问性一致性"></a>[UX-09 一致性](review/roadmap-ux-2026-09-10.md#ux-09-视觉文案与可访问性一致性) | 部分通过 | 新错误与字号反馈共存、正常终端、完整键盘和成功工具内容；随对应模块 |

<a id="ux-09-真窗口补验2026-09-10main--14ac19d4"></a>

UX-09 的 [9 月 10 日补验](review/roadmap-ux-2026-09-10.md#ux-09-真窗口补验2026-09-10main--14ac19d4) 已覆盖 MCP 空态、字号反馈收起与新普通反馈保留、长标题、长模型搜索及局部键盘；不能继续把这些场景全部列为未验，也不能扩张为错误共存、正常终端或全局键盘通过。

跨项仍缺（2026-09-14 更新）：真实 OAuth / 双账号额度切换。非空 MCP 已补齐（见 §2.2 末尾 2026-09-14 下午补录：服务器连接、工具发现、headless 与 GUI 内成功执行全链取证）。写入审批、非空 Changes、正常 PTY 已由 GUI2-07 批次补齐（审批卡三档、NOTES.md diff 与磁盘一致、PTY echo 往返）。它们是历史覆盖缺口，不等于已确认缺陷。保留旧记录的旁白待验事实，但遵循现有验收范围，不新增系统 VoiceOver 或完整无障碍合规门禁；键盘、AX 名称 / 状态 / 实际命中框继续验证。

## 4. 显示效果 Review（2026-09-12）与 GUI3 视觉任务

### 4.1 结论与证据边界

**结论**：GUI2-01～04 之后，Pawork 的空间合同（288px 侧栏、880px 阅读列、用户气泡靠右、工具默认折叠、按需 Inspector、独立 Settings、`Cmd+K` / `Cmd+F`）已与 Claude Desktop / Cursor Agent / Codex Desktop / PI / Zed 同构；**差距集中在视觉语言与信息设计**——工具行不可扫读、空闲 Composer 常驻负面文案、代码块与失败卡停留在「把字段画出来」、整窗用 Unicode 字形代替图标、侧栏每行都有状态圈且日期桶下重复项目头、Header 常驻动作对比度低、Diff 视图不像编辑器。这些都不需要重做壳层几何，多数也不需要改协议。

**证据**：GUI2-01～04 各轮保留在本机 `/tmp/pawork-gui2-0{1,2,3,4}-evidence/` 的真窗口截图（宽窗 / 1080×720、100% / 150%、工具展开、HTTP 401 失败卡、`Cmd+K`、`Cmd+F`、断线、Settings 供应商页）与当前主干源码；两份并行的模型只读审查（对话 / Composer 与壳层 / 侧栏 / 面板 / 设置各一份）已由主代理按源码逐条复核。审查中两处与截图时代不符的判断已剔除：GUI2-01 截图底栏「本轮用量 —」属 GUI2-02 之前的空闲底栏，当前源码 `run_status_visible` 只在运行中 / 待审批显示；「查找 · ⌘F」在 GUI2-04 截图中可见，问题是低对比文字型 Ghost 按钮，不是缺失。本节只登记设计判断与源码事实，**未运行 Cargo、未修改代码、未做新的真窗口验收**；不引用竞品像素值。

**红线核对**：所有 GUI3 任务保持纯 Rust / GPUI，`apps/desktop` 生产依赖仍只有 `pawork-client`；SVG 图标使用 gpui 0.2.2 内置 `svg()` / `AssetSource` / resvg（`Application::with_assets`，Desktop 当前未注册任何资产），不引入 rust-embed；不新增包、不改 wire / 持久化 / reducer。需要新 crate 或契约演进的候选单列于 §4.4，须用户确认后再立项。

### 4.2 发现清单

按优先级排列；「事实」为当前源码 / 截图，「差距」为一线 Agent UI 的通行做法，「归属」指向承接任务。

| 编号 | 区域 | 事实（源码 / 截图） | 差距 | 归属 |
| --- | --- | --- | --- | --- |
| R-01 | 工具调用 | 折叠标题由 `tool_group_summary` 生成「N 个工具 · M 已完成」，不含工具名与目标；展开后主行只画 `name`，参数 / 结果整段 pretty JSON 等宽堆叠（`timeline_entry.rs` `tool_group_summary` / `tool_detail_text` / `tool_row_element`）。相邻 ToolCall 之间一旦插入思考或正文即拆成多个「1 个工具」组，标题完全同质。共享 reducer 已提供 `ToolCall { name, status, detail, arguments }`；内建 8 个工具参数键稳定（`read_file` / `write_file` / `edit_file` / `apply_patch` / `list_directory` → `path`，`run_command` → `command`，`search_text` → `pattern` / `glob`，`find_files` → `pattern`）；MCP 工具回退显示名称。运行中指示为静态 accent 圆点 | Cursor / Claude Code / Zed 折叠态即「`Edit src/foo.rs`」「`Ran cargo test`」，展开是内容预览而非 JSON 文档 | GUI3-01 |
| R-02 | Composer | 无项目任务常驻「无项目」chip + 警告色「文件工具不可用」+「在项目中新建任务…」，目录缺 `context_window_tokens` 时右侧常驻「上下文 · 不可用」（`input_area.rs` 元信息行；`composer.file_tools_unavailable` / `composer.context_unavailable`）；已连接、有模型的空闲任务与空任务首页同样显示。模型触发器为 Ghost + `text.secondary` 文字 + `▾`，截图中像页脚元数据 | 一线产品空闲态不呈现故障文案；Context 只在有估算时出现；模型是可辨识 chip | GUI3-02 |
| R-03 | 代码块 / 链接 / 表格 | fence 语言已解析随即丢弃（`markdown.rs` `fence(line)` 返回 `_language`，`Block` 无 language 字段）；代码头只有右上「复制代码」Ghost 文字；无语法着色；链接在块下另排「打开链接 1 / 复制链接 1」文字按钮；表格每列 160px `min_w` 撑出空洞横滚，无表头底色 | ChatGPT / Claude / Cursor：代码头左语言、右悬停图标复制；链接行内可点；表头有底色 | GUI3-03（语法高亮见 §4.4） |
| R-04 | 失败卡 / Changes | Failed Run 摘要卡为 Ø40 圆 + ✕ +「运行失败」+ 原文「HTTP 401」，无可执行下一步（`run_summary_element`，注释明确不加假 retry）；Diff 行整行 `bg.panel` 仅 24px gutter 画 `+` / `-`，addition 复用按钮绿 `success_bg`，hunk 头未解析行号，文件行 44px + `▧` | Cursor / Codex 失败为紧凑 banner + 下一步（打开供应商设置）；GitHub / Zed diff 有左右行号与整行浅底 | GUI3-04；Diff 部分并入 GUI2-05 |
| R-05 | 图标体系 | 全窗 Unicode 字形：`⌕ + ▤ ◷ ⚙ ▾ ▸ ✎ ⋯ ◧ ↑ ✕ ↻ ▧ ✓ ● ○ ⟩ › ⌄`；`main.rs` `Application::new()` 未挂 `with_assets`，仓库内无 `svg()` 调用。1080×720 @ 150% 截图中侧栏「任务」与「+」之间的搜索钮几乎不可见；Settings chevron 曾因 12px 字形回退成圆点被迫改 20px | 一线产品统一单色 SVG 图标集，随 `text_color` 染色，各字号清晰 | GUI3-05 |
| R-06 | TaskRail | Timeline 分组 = 日期桶 → 项目头 → 任务，无项目时每个日期桶下都重复一行「未分组 N」（截图「今天 / 未分组 2」「更早 / 未分组 6」）；所有空闲任务强制画 Ø10 空心灰圆；桶头 36px 承载 12px 文字；150% 时行高 / 命中区仍是冻结 px，标题硬切、选中行改名 / 归档遮住标题 | Cursor / Claude / PI 的 Recents 直接按日期出任务行，空闲无图标、只用字重表达未读；分区头 11–12px 小号 | GUI3-06 |
| R-07 | Header / 断线 | 「查找 · ⌘F」「回合」为 36px 文字 Ghost 按钮、未设 `text_color`（`timeline_navigation.rs` `navigation_button`），与右侧 `⋯` / `◧` 图标风格不一致；branch / live 状态是裸文字，无 Git 时 Header 只剩标题。断线时侧栏插入全宽 Primary「重新连接」，主区 recovery 又有「重试连接」「连接诊断」两个 Raised 按钮，两处文案不一致；底部 12px 行把原始连接错误截成「connection is cl」 | 一线产品 Header 为图标动作 + chip；断线的解释与唯一主 CTA 在出问题的主列，侧栏只留静默状态 | GUI3-07 |
| R-08 | Settings | 供应商卡头 `px_4 py_4` 两列文字 + `▸`，名称与状态之间大片留白；八页导航纯文字无分组；Approvals 用 `●` / `○` 字形当 radio；Switch 关态 hover 走 `accent.hover`、滑块 `justify_end` 瞬移 | 可搜索 + 行式「名称 / 说明 / 值 / 动作」、品牌色点、分类图标 | 并入 GUI2-06 |
| R-09 | 层次 / 动效 | GUI3-08 已拉开近黑层次（canvas `#141416` / panel `#0e0e10`）；Switch 120ms 位移、流式静态 caret、Changes / Resources 骨架；hover / pressed 仍即时，不循环闪动 | Cursor / Zed / PI 的 surface 层差与有限动效 | GUI3-08 |
| R-10 | 首页 / 快捷查找 | 空任务首页「开始一个任务 / 在下方输入消息，开始这个任务。」与 Composer placeholder 重复，绑定项目的真入口藏在 11px 警告行；`Cmd+K` 结果无类型图标、高亮为 raised 灰、分组为说明句 | 一线产品欢迎态给出真能力入口；Command Palette 每行图标 + accent 高亮 | GUI3-02（首页）/ GUI3-05（图标） |

### 4.3 GUI3 视觉任务

与 GUI2-05～07 **写入集互不重叠**的项可穿插执行；`task_rail.rs` 由 GUI3-06 与 GUI3-07 先后触碰，两项串行。每项均为 Desktop 内改动，不改协议、持久化、共享 reducer 或 `apps/desktop` 生产依赖；触碰冻结视觉合同（TaskRail 行合同、Composer 元信息、Timeline 工具组）时同批回写 [GUI 设计](gui-design.md) 与 [Desktop Spec](spec/crates/desktop.md)。

| 任务 | 交付结果 | 依赖 / 顺序 | 实现 | 自动检查 | 代理真窗口 | 用户验收 / 归档 |
| --- | --- | --- | --- | --- | --- | --- |
| GUI3-01 | 工具调用可扫读 | 无；可与 GUI2-05 并行 | 已实现 | 233/233 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI3-02 | Composer 元信息、模型 chip 与首页文案 | 无；可与 GUI2-05 并行 | 已实现 | 233/233 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI3-03 | 代码块头、链接与表格 | 无 | 已实现 | 236/236 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI3-04 | 失败卡收窄与下一步 | 01 | 已实现 | 239/239 通过 | 通过（2026-09-14 非认证 banner + 401 CTA 与导航，见 §2.2 补录） | 待验 / 未归档 |
| GUI3-05 | SVG 图标体系 | 在 GUI2-05 / 06 之后（触达 inspector / settings） | 已实现 | 247/247 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI3-06 | 侧栏降噪与 150% 密度 | 无 | 已实现 | 235/235 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI3-07 | Header 动作与断线层级 | 06 | 已实现 | 236/236 通过 | 通过（2026-09-14，随 GUI2-07） | 待验 / 未归档 |
| GUI3-08 | 层次 token 与微动效 | 05；本轮已确认有限动效（不循环） | 已实现 | 248/248 通过 | 部分（2026-09-14 层次像素；动效留人工） | 待验 / 未归档 |

**推荐顺序**：GUI3-01 → GUI3-02 → GUI3-06 → GUI3-07 → GUI3-03 → GUI3-04 →（GUI2-05 / 06 完成后）GUI3-05 → GUI3-08。前四项改动面小、无新依赖、截图中最碍眼，先做；GUI3-05 触达面最广，等 Inspector / Settings 壳定型后一次替换，避免与 GUI2-05 / 06 抢同一文件。

#### GUI3-01 工具调用可扫读

- **改动**：新增 `tool_headline(name, arguments)`——内建 8 个工具按参数键抽取目标（`path` / `command` / `pattern`），其余与 MCP 工具回退为名称；单工具组折叠标题直接显示 headline（如 `write_file src/new_feature.rs`），多工具组显示数量 + 前 2–3 条 headline；展开态以 headline 作主行，参数 JSON 降为次级（缺 `path` / `command` 时才显示），结果保留首 8–12 行预览并可展开全文，「尚无结果 / 结果为空 / 空目录」诚实文案不变。运行中工具行加静态「进行中」词与 accent 点；当前 Run 的最后一条未闭合助手消息在作者行旁给出静态「正在生成」提示。短完成回合的用量收进「···」菜单或 hover，页脚只留终态词；失败仍保留原因卡。不用 `edit_file` 的 `old_string` / `new_string` 行数冒充 `+A / −D`。
- **写入集**：`apps/desktop/src/ui/{timeline_entry,timeline,i18n}.rs`、`apps/desktop/src/projection/timeline.rs`（工具组组装）、`ui/accessibility/app.rs`（tool group value 同源）。
- **验收**：真窗口折叠态可直接读出工具与目标；展开 / 折叠不丢阅读位置；AX `tool-group-toggle-*` value 含 headline；历史重放与 live 同源；MCP 工具与未知参数不崩不伪造。
- **自动检查**：`tool_headline` 单测（8 个内建工具 + MCP 回退 + 畸形 JSON）；既有 tool group 几何 / AX 回归更新断言。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：`timeline_entry.rs` 新增 `tool_headline` / `tool_headline_target`（内建 8 工具按 `path` / `command` / `pattern`（`search_text` 附 `glob`）抽目标，目标折叠空白并按 80 字符截断加 `…`；MCP / 缺键 / 畸形 JSON / 非字符串一律回退仅名称，`edit_file` 不读 `old_string` / `new_string`）。折叠标题 `tool_group_summary` 改为：单工具直接 headline（非全成功时追加状态计数，如 `run_command cargo test · 1 running`）；多工具为 `N 个工具 · 前 3 条 headline（超出加 …）· 状态计数`；AX `tool-group-toggle-*` value 与之同源。展开态每行以 headline 作主行，参数 JSON 仅在抽不出目标时作次级显示；结果保留首 10 行预览，超出时显示「展开全文 / 收起」按钮（展开键 `{event_id}:result` 复用 `expanded_timeline_details`，render / 测高 / AX 同源，AX identifier `tool-result-toggle-*`）；「尚无结果数据 / 结果为空 / 空目录」文案不变。运行中工具行状态词改为「进行中 / In progress」+ Ø14 accent 静态点；当前 active Run 最后一条非空助手消息、且该 Run 最新相位为 `run streaming_response` 时，作者行旁显示静态「正在生成 / Generating」（AX name `Pawork · Generating`）。终态页脚只留终态词 + 相对时间，用量 `run_usage_label` 移入该条目「···」菜单 Info 行（禁用、不可 Press）与按钮 tooltip；失败原因卡、取消轻量页脚不变。未改 `projection/timeline.rs`、协议、共享 reducer 与依赖。
- **自动检查**：Desktop 定向测试 233/233（基线 231 + 新增 `tool_headline_extracts_builtin_targets_and_falls_back`、`assistant_is_streaming_requires_active_run_and_streaming_phase`；更新 `tool_status_label_maps_succeeded_only`、`tool_row_view_from_parts_normalizes_detail`、`timeline_row_layouts_stack_with_content_heights_and_gaps`）。命令 `env -u CARGO_MAKEFLAGS cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，日志 `/tmp/pawork-gui3-01-tests.log`。子代理实现，主代理按源码逐 hunk 审查。
- **限制**：GPUI 0.2.2 `Div` 无 tooltip，用量未做页脚 hover，改走菜单 + 按钮 tooltip；「正在生成」依据公开事实（active_run_id + 最新 RunState 相位），未读 reducer 私有 committed 标记。代理真窗口 2026-09-14 随 GUI2-07 通过（工具 headline 折叠 / 展开、运行中状态词，证据 02 / 03 / 05）；用户验收未进行。

#### GUI3-02 Composer 元信息、模型 chip 与首页文案

- **改动**：去掉常驻警告色「文件工具不可用」，无项目时保留只读 chip + 一条正向入口（「绑定项目后可读写文件」→ 既有项目新建菜单）；Context 未知时不画，有 `context_window_tokens` 才显示 `— / {window}`；模型触发器改为 Raised chip、`text.primary`、保留 220px 槽与既有搜索菜单；空任务首页标题与 Composer placeholder 去重，可选一条 Ghost「绑定项目」复用 `on_project_task_menu`。不画 `@`、附件、slash、reasoning 芯片（§11 边界不变）。
- **写入集**：`apps/desktop/src/ui/{input_area,timeline,i18n}.rs`、`ui/accessibility/app.rs`（`composer-file-tools-hint` / `composer-context` 可见性同源）。
- **验收**：无项目 / 有项目 / 目录无 window / 断线四态下元信息行无负面常驻文案且入口可达；宽窄 × 三档字号不与 Timeline 末行重叠；模型 chip 命中区仍 ≥36px；草稿、IME、发送 gate 不变。
- **自动检查**：更新 Composer 实测布局 / AX 回归——无项目时无 warning 节点、无 window 时无 context 节点、chip 命中框。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：`input_area.rs` 元信息行去掉 warning 色 `composer-file-tools-hint`；无项目任务保留只读「无项目 / No project」chip + Ghost 正向入口「绑定项目后可读写文件 / Bind a project to read and write files」（`composer_project_task_label` render / AX 同源，Press 走既有 `on_project_task_menu`；断线按 `can_create_task` 禁用并给既有新建禁用 tooltip）；有项目任务显示 `Workspace · {name}` chip，不留入口空位。`composer_context_meter_visible`：当前生效模型目录缺 `context_window_tokens` 或为 0 哨兵时不渲染 `composer-context`，有 window 才显示 `Context · — / {window}`（`composer.context_unavailable` 词条保留但不再常驻）。模型触发器改 `ButtonVariant::Raised` + `text.primary`，保留 220px 槽 / 36px 高 / 长名截断 / `▾` / 搜索菜单 / 禁用 tooltip / AX `model-picker`。`timeline.rs` 空任务首页只留标题「开始一个任务」，去掉与 placeholder 重复的提示（`welcome_hint_visible` 仅无 active session 时显示「从侧栏选择或新建」）；空任务且无项目、已连接时显示 Ghost「绑定项目 / Bind a project」（`workspace-bind-project`，独立 `welcome_bind_project_focus` 句柄，复用 `on_project_task_menu`）。无任务首页 Primary `New task` 不变。未画 `@` / 附件 / slash / reasoning；草稿、IME、发送 gate、模型菜单不变。
- **自动检查**：Desktop 定向测试 233/233（更新 `composer_layout_keeps_controls_in_card_and_ax_aligned`、`project_task_guidance_preserves_context_and_wraps`、`welcome_and_rail_follow_actual_layout`：无项目无 warning 节点且有正向入口、无 window 无 `composer-context`、chip 命中框、首页无重复提示）。日志 `/tmp/pawork-gui3-02-tests.log`。主代理审查发现欢迎态按钮复用 `header_new_task_focus`（Inspector 展开 + 空任务时与 Header「+」双焦点），已改独立句柄后复测通过。
- **限制**：无；`composer_layouts["composer-file-tools-hint"]` 死槽已于 2026-09-14 收口删除（`mod.rs` 布局表单行移除，Desktop 定向测试 261/261，收口 cargo check -p pawork --offline 通过）。代理真窗口 2026-09-14 随 GUI2-07 通过（首页标题去重、工作区 chip、模型 Raised chip 随名收缩，证据 01 / 02 / 05）；用户验收未进行。

#### GUI3-03 代码块头、链接与表格

- **改动**：`Block` 增 `language` 字段，代码头左侧显示语言标签、右侧悬停 / 聚焦显示图标复制（GUI3-05 前先用现有字形）；行内链接直接可点（已有 underline），块下「打开链接 N / 复制链接 N」文字按钮移入消息「···」菜单（`message_actions` 已生成）；表格表头加 `surface.hover` 底色，去掉每列 160px `min_w`，按内容测宽再横滚。复制仍取原始内容并保留换行。
- **写入集**：`apps/desktop/src/ui/{markdown,timeline_entry,i18n}.rs`、行数估算 `message_block_line_counts` 与 AX。
- **验收**：中英文 / 代码 / 表格 / 长链接不裁切；复制粘回与原文一致；链接动作经菜单与键盘可达；三档字号下代码头不与正文重叠。
- **自动检查**：`markdown.rs` parse golden 增 language 字段；复制、链接与表格既有测试更新。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：`markdown.rs` `Block` 增 `language: Option<String>`（`fence_language` 取 info string 首个空白词）。代码块顶部 36px 代码头：左侧 12px `text.secondary` 语言标签（无 info string 显示「代码 / Code」），右侧 36×36 Ghost 图标复制按钮（字形 `⧉`，tooltip「复制代码 / Copy code」，默认透明、悬停代码块 `group("markdown-code")` 或按钮聚焦时显现，Tab / AX 始终可达，AX identifier `{event_id}-code-{index}` 可 Press），复制仍取原始 `block.code` 含换行。行内 HTTP(S) 链接以 `InteractiveText` 包 `StyledText` 直接可点（保留 underline，`cx.open_url`，未知协议不可点）；块下「打开链接 N / 复制链接 N」文字按钮移除，动作留在消息「···」菜单，菜单行文案改为 `Open link N · {去 scheme 截断 32 字 URL}`。表格表头加 `surface.hover` 底色，去掉每列 160px `min_w`，列宽 = 该列最长单元格 `shape_line` 实测宽 + 12px，超出阅读列仍 `overflow_x_scroll`。`message_block_line_counts` 代码头计 1 行、移除链接动作行、表格按行计不按 160 折行；`timeline.rs` `message_entry_height` 每个代码块补 `max(0, 36 − 行高)` 与代码头同源。未做语法高亮、未加 crate。
- **自动检查**：Desktop 定向测试 236/236（更新 `markdown_blocks_and_visible_inline_text_share_line_counts`（language 有 / 无 / 带空格 info string、列宽非 160、行数）、`streaming_unicode_and_unclosed_markers_preserve_content`、`reply_actions_copy_exact_content_and_use_closed_turn_boundary`（代码复制 AX Press、链接动作在菜单不在正文）、`localize_covers_both_languages_and_unknown_key`）。日志 `/tmp/pawork-gui3-03-tests.log`。
- **限制 / 偏离**：行内链接为鼠标命中，未给每条链接单独 AX 节点（键盘 / AX 走菜单）；渲染列宽用 `text_system` 实测而 golden 以字符 × 0.6 估算断言「非 160」；无语言时显示「代码」而非留空。代理真窗口 2026-09-14 随 GUI2-07 通过（rust 代码头语言标签 / 复制图标、真实回合表格表头底色与内容列宽，证据 02 / 25）；用户验收未进行。

#### GUI3-04 失败卡收窄与下一步

- **改动**：Failed Run 摘要卡收为紧凑 banner（状态圆 Ø40 → Ø20、单行标题 + 原因）；原因匹配 401 / 403 / authentication 时提供「打开供应商设置」CTA（复用 `on_manage_composer_models`），其余失败只提供既有诊断入口；不加假 retry。Cancelled 仍只留轻量页脚。
- **写入集**：`apps/desktop/src/ui/{timeline_entry,i18n}.rs`、`apps/desktop/src/projection/timeline.rs`（原因文案）、`run_summary_card_height` 与 AX 同源公式。
- **验收**：真实 `opencode-go / glm-5.3-flash` 401 终态下卡片高度、CTA 可达、进入 Providers 返回后任务 / 草稿保留；成功 / 取消态不受影响。
- **自动检查**：AX summary 几何回归增失败卡高度与 CTA identifier；401 分类为 UI 启发式单测。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：`timeline_entry.rs` 失败 Run 摘要卡收为紧凑 banner——状态圆 `SUMMARY_STATUS_CIRCLE` Ø20（字形 13px），水平 12px / 纵向 `py_3` 内边距，单行标题「运行失败 / Run failed」+ `BODY_SM` 原因原文自然换行不截断；新增纯函数 `failure_next_step(reason)`（大小写不敏感子串匹配 `401` / `403` / `authentication` / `unauthorized` / `invalid api key` → `OpenProviderSettings`，其余 `None`），命中时 banner 内提供 Raised「打开供应商设置 / Open provider settings」（identifier `run-open-providers-<event_id>`，mouse / Enter / Space / AX Press 复用 `on_manage_composer_models` 进入 Settings → Providers，返回保留任务 / 草稿 / Run；断线与模型菜单进入 Settings 同 gate，Host 数据 stale）；不加假 retry，非认证失败无新按钮。Completed 有可审阅 Changes 的卡同步 Ø20 + 紧凑 padding，`Review changes` Primary 168×40 与 gate / identifier 不变；Cancelled 仍只留页脚。`timeline.rs` 新增 `RunSummaryCardLayout` / `run_summary_card_layout` / `run_summary_card_height`（render / 测高 / AX 同源：`2 × 0.75rem + max(20, 标题行高) + 0.5rem + 原因行数 × 行高 +（CTA 时）0.5rem + 28`；100% 下 `HTTP 401` + CTA = 113px）。`ui/mod.rs` 增 `timeline_open_providers_focus` 懒建句柄。`accessibility/app.rs` 失败卡高度走同一 layout，CTA 节点可 Press。`projection/timeline.rs` 未改。
- **自动检查**：Desktop 定向测试 239/239（新增 `failure_next_step_classifies_auth_keywords`、`run_summary_card_height_uses_compact_banner_formula`、`failed_run_banner_geometry_and_provider_cta_preserves_draft`：失败卡高度、CTA Press → Providers → 返回草稿保留、timeout 无 CTA、成功 Review 仍在、取消只有页脚）。日志 `/tmp/pawork-gui3-04-tests.log`。
- **限制**：分类纯函数位于 `timeline_entry.rs` 而非 `projection/`；`theme.rs` 旧 `SUMMARY_CHECK_CIRCLE=40` 常量保留未删（不在写入集，摘要卡已改用新常量）。真实失败紧凑 banner 2026-09-14 随 GUI2-07 取到（Host 中途终止的 `run_failed`，非认证原因正确无 CTA，证据 23）；401 终态 CTA 当日下午补齐（§2.2 补录，含 CTA 导航取证）；用户验收未进行。

#### GUI3-05 SVG 图标体系

- **改动**：`apps/desktop` 实现最小 `AssetSource`（`include_bytes!` 单色 SVG，路径 `icons/*.svg`），`main.rs` 改 `Application::new().with_assets(...)`；新增 `ui/components/icon.rs` 封装 `svg().path().text_color()` 与 20px 字形尺寸；分两批替换字形——先 Composer / Timeline / TaskRail / Header（Send、Cancel、Search、New、Grouping、Settings、Chevron、More、Copy、Back-to-bottom、Find），再 Inspector / Changes / Resources / Settings（Refresh、Collapse、File、Radio、Check）。AX name 继续走 i18n 文案，不以字形作 name。`Cmd+K` 结果行加类型图标、高亮改 `surface.hover` + 左侧 2px accent。
- **写入集**：`apps/desktop/src/main.rs`、`ui/components/icon.rs`（新）、`ui/{input_area,task_rail,timeline_entry,timeline_navigation,quick_search,inspector,changes,resources}.rs`、`ui/settings/*`、`ui/mod.rs` Header；图标资产目录置于 `apps/desktop/assets/icons/`。
- **验收**：1440×1024 与 1080×720、100% / 125% / 150% 下全部动作图标清晰可辨且命中区不变；AX identifier / name 不漂；`--probe` 冒烟不回退。
- **自动检查**：`AssetSource` 能加载全部登记路径的单测；既有 AX 回归保持绿；150% 窄窗侧栏搜索 / Header Find 的 bounds 非零。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：新增 `apps/desktop/assets/icons/` 41 个手写单色 SVG（viewBox 0 0 20 20、stroke 1.5 / fill、无 `<style>` / 外部引用、非第三方图标集拷贝）与 `ui/components/icon.rs`：`Icon` 枚举 41 项（Send / Cancel / Search / Find / Plus / Grouping / Clock / Settings / ChevronDown·Right·Left / More / Copy / ArrowDown·Up / Refresh / Collapse / File / Check / RadioDot / RadioRing / Link / Edit / Archive / Inspector / Terminal / Changes / Resources / Turns / Branch / Task / Page / SettingsRow / Providers / Network / Approvals / Tools / Appearance / Advanced / About / Project）、`Icon::path()` → `icons/<kebab>.svg`、`icon()`（20px `flex_none`）/ `icon_sized()`（16px chip / 行内），颜色由调用方 `text_color` 染色；`Assets` 以 `include_bytes!` 静态表实现 gpui `AssetSource`（`load` / `list("icons")`），`main.rs` 改 `Application::new().with_assets(ui::Assets)`，不引入 rust-embed。`theme.rs` 仅新增 `ICON_SIZE=20` / `ICON_SM=16`，命中区常量不变。字形替换：Header（`+` / `⋯` / `◧` / `⑂`→Plus / More / Inspector / Branch）、Composer（Send / Cancel、chip `▾`、菜单勾）、TaskRail（Grouping / Clock / Search / Plus / Settings / Chevron / Edit / Archive）、Timeline（More / 工具 Check / Chevron / 摘要 Check·Cancel / 回底 ArrowDown）、Find（Find / Turns / ArrowUp·Down / Cancel）、Markdown 代码头 Copy、Inspector（ChevronDown / Collapse / 终端回底）、Changes / Resources `↻`→Refresh 16px、Settings 八页导航 16px 图标（Providers / Network / Approvals / Tools / Terminal / Appearance / Advanced / About）、供应商卡头 chevron。`Cmd+K` 结果行左侧 16px 类型图标（Task / Page / SettingsRow；面板行 Changes / Terminal / Resources），选中态改 `surface.hover` + 绝对定位 2px accent 左条（行高与 AX 不变）。AX identifier / name、`APP_VIEW_KEYBINDINGS` / `install_keybindings` / `MAIN_PATH_TAB_STOP_IDS` 与所有焦点句柄未动；`TestAppContext` asset source 为 `()`，svg 在测试中只占位不渲染（gpui 对 `load → None` 静默跳过）。
- **自动检查**：Desktop 定向测试 247/247（新增 `assets_load_every_registered_icon`：遍历 `Icon::all()` 均 `load → Some` 且以 `<svg` 开头、`list("icons")` 数量一致、路径无重复；既有 AX / 布局回归全绿）。日志 `/tmp/pawork-gui3-05-tests.log`。子代理顺手 rustfmt 了 `platform/preferences.rs` / `approval_card.rs` / `components/dropdown.rs` 三处纯格式改动，主代理已还原。
- **限制 / 偏离**：Approvals radio 保留 GUI2-06 的几何圆环未换 svg；`dropdown.rs` MenuRow `✓`、Settings 各页「刷新」文字按钮、`timeline.back_to_bottom` 文案中的 `↓`、取消态摘要圆的 `—` 未改；`File` / `Link` / `RadioDot` / `RadioRing` / `Project` 已登记未上屏；`font::ICON` rem 常量保留但渲染侧不再引用。真窗口图标渲染 2026-09-14 随 GUI2-07 通过（100% 全批与 150% 20～22 清晰可辨、Cmd+K 类型图标 24）；`--probe` 冒烟 2026-09-12 已跑；用户验收未进行。

#### GUI3-06 侧栏降噪与 150% 密度

- **改动**：日期桶内若只有一个项目组且为 Unassigned（或等于当前筛选项目）则跳过项目头，任务直接挂桶下；`SessionLiveStatus` 为空时不画状态点，仅 Needs input / Running / Blocked 出点，未读继续只用 SEMIBOLD；桶头 36px 收为 24px；150% 时任务行改 36px、相对时间仅悬停 / 聚焦显示，改名 / 归档不遮标题；标题截断槽右侧叠 `linear_gradient` 渐隐。项目分组视图与 grouping / scope 正交语义不变。
- **写入集**：`apps/desktop/src/ui/{task_rail,theme,i18n}.rs`、`apps/desktop/src/projection/session.rs`（分组组装）、`ui/accessibility/app.rs`；回写 [gui-design §4](gui-design.md#4-工作台壳与首页) TaskRail 行合同与 Desktop Spec §3.2。
- **验收**：单项目 / 多项目 / 无项目三种数据下分组头无冗余、active 任务仍可滚入；状态点只在有语义时出现；150% 窄窗任务标题可读、动作不遮挡；改名 / 归档 / 撤销路径不变。
- **自动检查**：TaskRail AX 几何回归——单项目 Timeline 下每桶 0 个 `project-*` 头、空闲行无 status-dot 节点、150% 标题 frame 宽 > 80px。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：`projection/session.rs` 新增 `TaskRailDateGroup::skip_project_header(scope)`——仅 Timeline 分组、桶内恰一个项目组且为 Unassigned 或等于当前 scope 时跳过项目头（含定向「+」），任务直接挂桶下且不受折叠影响；Projects 视图与多项目桶保持头 / 计数 / 折叠 / 「+」。`task_rail.rs`：`SessionLiveStatus` 为空不画状态点也不占位，仅 Needs input（琥珀）/ Running（accent）/ Blocked（danger）出实心点，unread 仍只用 SEMIBOLD；标题截断槽右侧叠 16px `linear_gradient(90°)` 渐隐，颜色与行底（panel / raised / hover）同源。`theme.rs`：`RAIL_BUCKET_HEADER_HEIGHT` 36 → 24；新增 `RAIL_TASK_ROW_HEIGHT_COMPACT`=36、`rail_task_row_height(TextScale)`（仅 150% 为 36，100% / 125% 仍 44；项目头仍 44）、`rail_session_title_layout`（render / AX 共用标题槽起点与宽度）。150% 空闲行不保留 64px 尾槽、宽度还给标题；悬停 / 行或动作聚焦 / 当前会话时露出改名 / 归档且不遮标题。`ui/mod.rs` `rail_focus_stops` 与跳过头同源（跳过时不进 ProjectHeader / ProjectAdd 档）。`accessibility/app.rs` `project_ax_nodes(.., skip_header)`：头 / 行高 / 状态点 / 标题框同源，新增 `session-title-*` 节点，空闲行不发布 `status-dot-*`。
- **自动检查**：Desktop 定向测试 235/235（新增 `timeline_skips_redundant_project_header_for_unassigned_or_scoped_singleton`（投影）、`task_rail_skips_redundant_headers_and_compacts_150`（AX 几何：单项目 / Unassigned 桶 0 个 `project-*` 头、多项目桶有头、空闲无 status-dot 而 Running 有、桶头 24、100% 44 / 150% 36、150% 标题 > 80px 且不与动作重叠）；更新 `task_rail_geometry_and_font_constants_match_frozen_tiers`、`rail_focus_stops_follow_design_tab_order`）。日志 `/tmp/pawork-gui3-06-tests.log`。子代理顺手 rustfmt 了 `approval_card.rs` / `dropdown.rs` / `settings/providers.rs` 三处纯格式改动，主代理已还原以守最小写入集。
- **限制**：150% 下相对时间与动作共用 `session_actions_visible` 谓词且动作优先，因此 150% 实际不显示 `now / Nm` 字串（空闲行把宽度还给标题）；若要求「悬停先显时间」需改谓词。代理真窗口 2026-09-14 随 GUI2-07 通过（分组头 / 状态点语义、150% 窄窗密度，证据 12～16 / 21）；用户验收未进行。

#### GUI3-07 Header 动作与断线层级

- **改动**：Find / 回合入口改 36×36 图标按钮（GUI3-05 前先用字形 + 强制 `text.secondary`），tooltip 承载「⌘F」；branch / live 状态改带底 chip，无 Git 时不留空洞；Inspector 重开入口保留待 GUI2-05 整理。断线时侧栏 Primary「重新连接」降为状态行 Ghost「重试」，唯一 Primary 留在主区 recovery；统一「重新连接 / 重试连接」文案；底部 Local 行只显示「本地 · 已断开」，原始错误留在 recovery 技术详情。
- **写入集**：`apps/desktop/src/ui/{timeline_navigation,task_rail,recovery,i18n}.rs`、`ui/mod.rs` Header 装配、AX。
- **验收**：有 / 无 Git、有 / 无任务、断线三态 Header 均无空洞与低对比动作；断线草稿保留、重试 gate 与 UX-04 一致；`reconnect` Tab 停靠链不变。
- **自动检查**：`timeline-find` 在 1440 与 1080@150% 的 AX frame 宽 > 24px 且不与 `inspector-expand` 重叠；断线时 `reconnect` 与 `connection-retry` 层级不同；底行文案不被裁切。

**本轮进展（2026-09-12，`main / f181bae3` 工作区候选）**：`timeline_navigation.rs` 新增 `navigation_header_icon_button`——Header 查找 / 回合入口改为 36×36 Ghost 图标按钮（`⌕` / `≡`，`font::ICON` 20px，`text.emphasis`），tooltip 与 AX name 走 i18n「查找 · ⌘F」/「回合」，identifier `timeline-find` / `timeline-turns` 与键盘 / 回焦不变；查找栏内文字按钮不变。`ui/mod.rs` Header branch / live 状态改 `header_meta_chip`（Raised 底、12px、r6、pad 6/4；无 branch 且无 live 状态不渲染、不留 gap），`inspector-expand` 保留待 GUI2-05。断线层级：`task_rail.rs` 去掉列表上方全宽 Primary「重新连接」，`reconnect` 下沉为底部 Local 行 Ghost「重试 / Retry」（tooltip「重试连接 / Retry connection」，identifier / -17 Tab 档 / handler / gate 不变）；`recovery.rs` `connection-retry` 改唯一 Primary「重试连接 / Retry connection」，其余恢复按钮仍 Raised；新增 `connection_error_detail`，原始连接错误在主区 recovery 内换行完整显示并进入 `connection-notice` AX value。底部 Local 行 `connection_status_label` 不再嵌入原始错误：`Local · Disconnected / 本地 · 已断开`、`Local · Connecting / 本地 · 连接中`，已连接沿用既有文案。`theme.rs` 新增 `HEADER_CHIP_RADIUS` / `HEADER_CHIP_PAD_X` / `HEADER_CHIP_PAD_Y`。AX：chip 与 reconnect 几何读实测 layout（`header-branch` / `header-status` / `rail-reconnect-layout`），`reconnect` description `ghost`、`connection-retry` description `primary`。
- **自动检查**：Desktop 定向测试 236/236（新增 `header_actions_and_disconnect_hierarchy`：1440×1024 与 1080×720@150% 下 `timeline-find` 宽 ≥ 36px 且不与 `inspector-expand` 重叠、无 Git 无 live 状态无 `header-branch` / `header-status` 节点、断线时 `reconnect` Ghost 与 `connection-retry` Primary 并存、底行等于 `Local · Disconnected` 且框在 rail 内；更新 `recovery_keeps_errors_local_and_preserves_drafts`）。日志 `/tmp/pawork-gui3-07-tests.log`。
- **限制 / 偏离**：连接原文在 recovery 中始终可见换行，未另加「展开技术详情」折叠；回合字形用 `≡`（`▤` 已被侧栏 grouping 占用），待 GUI3-05 换 SVG；Settings Advanced 的「重新连接」文案不在写入集未统一。代理真窗口 2026-09-14 随 GUI2-07 通过（Header 图标动作、断线层级与底行「本地 · 已断开」，证据 05 / 12）；用户验收未进行。

#### GUI3-08 层次 token 与微动效

- **改动**：对照 PI Desktop 暗色截图（近黑画布、胶囊 Composer、安静空态）与 Zed / gpui `with_animation` API，拉开 canvas / panel 层次并同步 AA 测试；Composer 16px 圆角 + 轻阴影 + 聚焦 accent 描边；首页空态加装饰 Task 图标；流式助手最后一行末尾加静态 2×14 caret（不循环，避免 Reduce Motion 分支）；Switch 滑块仅在状态切换时 120ms ease-out，首帧瞬移；Changes / Resources 加载保留标题并加静态骨架行。hover / pressed / focus 仍即时。不做菜单淡入（测试首帧透明风险）、不做浅色主题、不复制 PI 角色插画。
- **写入集**：`apps/desktop/src/ui/{theme,timeline,timeline_entry,markdown,input_area,changes,resources}.rs`、`ui/components/{switch,skeleton,mod}.rs`；回写 gui-design §3.2 与 Desktop Spec 平台显示偏好一节。
- **限制**：caret 循环与 Reduce Motion 读取仍不立项；菜单 / 骨架不做循环闪动。

#### GUI4 壳层收口（StatusBar 三栏与共享空状态）

- **改动**：底部 30px StatusBar 分成三栏：左项目 / 权威 branch（无数据不占空洞）、中用量（仍绝对居中，AX 只发 `run-status`）、右优先 `status_hint` 否则字号反馈否则连接文案。字号与普通操作提示不再挂 Composer，筛选说明仍挂 Composer。新增共享 `EmptyState`，首页 / Changes / Resources 空态与加载骨架走同一居中容器；identifier 仍由调用方 shell 节点承担。
- **写入集**：`apps/desktop/src/ui/{mod,input_area,timeline,changes,resources,theme,accessibility/app}.rs`、`ui/components/{empty_state,status_bar,mod}.rs`；回写 [GUI 设计](gui-design.md) 与 [Desktop Spec](spec/crates/desktop.md)。
- **限制 / 偏离**：左右栏视觉不进 AX；字号反馈 3 秒收起不清除同时到达的 `status_hint`；Settings 壳仍不显示 StatusBar。真窗口与用户验收未进行。

#### GUI4 终端显示层（行覆盖、SGR、按键直通与自动 resize）

- **改动**：Inspector Terminal 从剥 CSI 后把 CR 当成换行的纯文本，改为行缓冲显示层：CR 覆盖同一行、退格回退光标、EL 擦行、SGR 16 色进 `StyledText`。Ctrl-C / Tab / ↑↓←→ 在输入框聚焦时直通 `terminal_write`；可打印字符仍走行式 Enter，空行也发换行。未启动时带正文的 Enter 会在 create 回执后补发。输出面视口像素自动 `terminal_resize`（扣内边距、随字号缩放，20–500×6–200），stepper 草稿优先；切标签后把当前面板尺寸套到新会话。CUP / CUU / CUD 丢弃，不声称网格 / alt-screen / vim。不改 wire、不直连 PTY、不新增 crate。
- **写入集**：`apps/desktop/src/ui/{terminal_view,inspector,mod}.rs`、`apps/desktop/src/projection/terminal.rs`；回写 [GUI 设计](gui-design.md)、[Desktop Spec](spec/desktop.md)、[desktop crate Spec](spec/crates/desktop.md)、[AGENTS.md](../AGENTS.md)。
- **限制 / 偏离**：输入仍在输出面底部 TextInput，不是网格内字符级编辑；无选中复制、无完整 VT 仿真。真窗口与用户验收未进行。
- **后续（2026-09-15，用户确认新增包）**：终端显示核心抽为 `pawork-terminal`（crates/terminal，零依赖纯库）：行缓冲解析、SGR 属性分段、按键→PTY 字节、面板像素→列×行估算与对应测试迁入；`ui/terminal_view.rs` 收缩为 gpui 胶水层（主题色 runs、Keystroke 转换、resize 防抖）。Desktop 生产依赖白名单改为 {pawork-client, pawork-terminal}；不改 wire / Host / 面板行为。

### 4.4 需用户确认的候选（不随 GUI3 自动立项）

| 候选 | 原因 |
| --- | --- |
| 代码块语法高亮 | 需新增 crate（`syntect` 或 tree-sitter 系）；触碰 `apps/desktop` 依赖 deny-list 边界与虚拟列表测高；GUI3-03 先交付语言标签，颜色由 `StyledText` `TextRun.color` 即可承载，不需新渲染器 |
| 工具真实 `+A / −D` | `edit_file` 结果只报 segment 数，结构化 metadata 未进 `TimelineItem`；需契约演进（`ToolResult.metadata` 投影到查询 / 事件），golden 先行 |
| 失败后重发同一 prompt | 需核对 RunStart 能否携带既有消息重放；无则属契约演进，GUI3-04 不做假 retry |
| 正文字符级查找高亮 | GUI2-04 已明确推迟，等查找与回合目录验收后再评估 |
| 品牌色 / 供应商 logo | 无权威来源不画（与 [gui-design §4](gui-design.md#4-工作台壳与首页) 侧栏「无权威来源元素不画」一致）；GUI2-06 只用中性状态色点 |

<a id="4-验证与交付规则"></a>

## 5. 验证与交付规则

文档变更只检查相对链接、锚点、状态与 diff。实现任务先跑相关已有定向检查；Desktop 为 bin-only，命令为：

```bash
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders
```

需要启动最终候选时才构建 Desktop；实际修改其他包或契约才补该包必要测试 / golden。全会话保持一个 Cargo 进程，复用 `target/`，不做全工作区门禁。新行为确无覆盖才补最小回归，不增加截图流水线、框架或测试基建。

真窗口按受影响路径检查 1440×1024、1080×720 与 100% / 125% / 150%；涉及 Inspector 时加检查阈值附近开合。每项记录候选、命令、事实证据和阻塞，截图存任务证据目录，不检入仓库。实际捕捉与运行结果不能由历史日志替代。

Validated: 2026-09-12 本次仅文档变更——§4 显示效果 Review 与 GUI3 登记经相对链接 / 锚点检查与 `git diff --check`，未运行 Cargo、未改代码。此前 GUI2-04：Desktop 定向测试 231/231、Desktop 构建、上述真窗口路径（长对话查找循环 / 回合目录定位 / 无结果 / Esc 回焦 / 草稿保留 / 150% 字号）、git diff --check；GUI2-01 / 02 / 03 证据保留于本项记录。

Targeted regressions: 本次 none（文档）。GUI2-04：当前对话查找 / 身份定位 / 回合目录一条新增主路径回归；既有 230 项 Desktop 回归全部通过。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
