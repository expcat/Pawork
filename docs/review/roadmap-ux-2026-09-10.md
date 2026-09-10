# Pawork GUI 用户体验收口：UX-01～UX-09 历史记录

> 2026-09-10 从活动 ROADMAP 原文迁存，仅调整标题与相对链接。迁存不代表用户验收或任务归档；未闭合项由 [当前路线图](../ROADMAP.md#3-前阶段验收承接) 继续跟踪。以下日期、候选、测试与窗口结论均属于原批次，本次未重跑。

> 2026-09-10；基线 `main / ac6578d2`。本线来自当前构建的真实窗口走查，详见 [GUI 验收报告](gui-ux-audit-2026-09-09.md)。**核心对话可用，整体验收不通过；UX-01 已实现并通过定向自动检查，真窗口已覆盖主要编辑场景，系统 IME 与用户验收待补齐；UX-02 已实现并通过定向自动检查与历史回复真窗口复验，新请求流式验收因 HTTP 401 待补齐；UX-03 已实现并通过定向自动检查与代理真窗口复验，用户验收待进行；UX-04 已实现并通过定向自动检查与代理真窗口复验，用户验收待进行；UX-05 已实现并通过定向自动检查，代理真窗口部分通过（后续操作被屏幕捕捉故障阻断）；UX-06 已实现并通过定向自动检查，代理真窗口部分通过（真实额度查询不可用，数值与倒计时待补验）；UX-07 已实现并通过定向自动检查，代理真窗口部分通过（真实请求 HTTP 401，成功工具与非零用量待补验）；UX-08 已实现并通过定向自动检查与代理真窗口复验，用户验收待进行；UX-09 已实现并通过定向自动检查，代理真窗口部分通过（新错误共存、完整长模型矩阵与旁白待补验）。** 本次代理受托按用户视角验收，不代表用户本人已经签字。

旧 UI-1～UI-6（含多账号 G1、额度 G2）的实现任务和长篇完成日志已从本文件移除，移存 [历史记录](roadmap-ui-2026-09-09.md)。历史人工验收状态原样保留；本线只修本次发现的剩余问题，不重做已完成能力。

## 1. 执行顺序与完成标准

先完成 P1 输入、阅读、项目引导与错误恢复，再处理 P2 查找效率、信息表达和视觉一致性。每项只改相关模块；沿用当前 Rust / GPUI 架构、既有 token、事件与权限语义。

每项状态分别记录：**已实现 → 相关定向检查通过 → 代理真窗口复验通过 → 用户验收**。不得用截图、模型回复里的“通过”或历史测试日志代替真实功能结果。涉及冻结 wire / schema 时按 [工程约定](../../AGENTS.md) 单独确认，优先复用已有字段。

## 2. P1：先消除主路径障碍

### UX-01 长草稿自然换行与编辑

- **问题 / 证据**：长中文整段粘贴后只显示一行并在右侧裁掉，显式换行才增高；输入前无法完整审阅。报告 F01，截图 04 / 29。
- **最小任务**：Composer 按可用宽度自然换行；让光标、选择、中文输入法、鼠标定位和滚动使用同一换行结果；保持原来的高度上限与逐任务草稿。
- **验收**：长中文段落、长路径和显式多行在三档字号下可完整编辑；窄窗无需横向追字；Shift+Enter、撤销、任务切换恢复与 IME 组合输入不误发。
- **范围**：Desktop `ui/text_input.rs` 与 Composer 装配；对应 Desktop Spec。状态：**已实现；定向自动检查通过；代理真窗口部分通过（系统 IME 待补验）；用户验收待进行；未归档。**

#### UX-01 本批证据（2026-09-09）

- **实现**：Composer 在确定宽度后计算显示行，完整 grapheme 不拆分，软换行不改原文；测高、绘制、选择、鼠标与 IME 坐标共用显示行和 byte 起点。保持 220px 卡片上限；AX 使用实际输入视口高度。修正组合输入选区相对新组合文字的 UTF-16 偏移，以及放大字号 / 缩窄窗口后滚动范围未更新导致光标未跟随的问题。终端、secure 与只读 URL 保留既有策略；无 wire / schema / 依赖变化。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，220 passed / 0 failed；`cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。新增一个换行主路径回归，覆盖三档字号、宽窄输入、中文 / 无空格长路径 / emoji 与组合字符 / 显式空行、原文完整性、滚动、鼠标 / IME 坐标往返、组合输入更新和单次撤销；复用 Composer 实际布局 / AX 测试及既有键位、IME、草稿、secure / 只读输入回归。
- **代理真窗口已通过部分**：独立最终候选连接隔离只读 Host，验证长中文与长路径自然换行，100% / 125% / 150% 与拖窄窗口、字号变化后末尾及光标可见、Home / End、Shift 选择、鼠标中段插入与撤销、Shift+Enter、滚动阅读、两个任务来回切换恢复各自草稿。窗口截图见本次任务工具记录，不检入仓库。
- **仍待补验**：系统中文 IME 候选与 composing Return 的真窗口操作。`Ctrl+Space` / `Ctrl+Alt+Space` 后仅录入普通 `ni`，未出现候选框；系统菜单入口也未能经当前 CUA 获取。自动 `EntityInputHandler` 回归已通过，但不能替代系统 IME 结论。用户验收、归档均未完成。
- **候选与窗口外事实**：`target/pawork-desktop-runtime/Pawork-UX01-final.app`，SHA-256 `e8fbf169013efad12ad7134a9710d324ff717e652c7a3610bee0a87f8e4c10f4`，与最终 build 相同。实际 argv 指向 `/tmp/pawork-ux01/data/pawork-gui-ux01.sock`；Host 使用当次 `opencode-go / glm-5.3-flash / read-only` 参数。只读 SQLite 核对为 2 个测试 session、0 Run、0 session event；没有发送或调用 Provider。偏好恢复 `zh / 100%`。
- **检查记录**：`/tmp/pawork-ux01-desktop-final-tests.log`、`/tmp/pawork-ux01-build.log`、`/tmp/pawork-ux01/final-evidence.json`；复用既有临时 rustc 索引 wrapper 与测试 runner，单 Cargo 进程、无 clean。Rust 格式、文档本地链接、`git diff --check` 通过。未提交、推送或发布。

### UX-02 回复可阅读、可复制、可使用

- **问题 / 证据**：标准 Markdown 表格显示原始竖线和分隔行；代码块无复制动作；已完成回复的菜单只有禁用的“分叉”。报告 F02，截图 06 / 18 / 19。
- **最小任务**：补齐常用表格呈现、回复复制与代码复制；链接提供可发现的打开 / 复制动作；分叉靠近可用的闭合回合边界，并说明禁用原因。
- **验收**：同一份含中文表格、代码块、长链接的回复在流式完成与重开任务后均可读；复制内容完整且不夹入作者 / 时间；长内容可滚动，不遮住 Composer。
- **范围**：Desktop `ui/markdown.rs`、`ui/timeline_entry.rs` 与相应动作 / AX。状态：**已实现；定向自动检查通过；代理真窗口部分通过（新回复流式完成待补验）；用户验收待进行；未归档。**

#### UX-02 本批证据（2026-09-09）

- **实现**：常用管线表格支持表头、左右 / 居中对齐、中文、escaped pipe 与行内代码；未完成分隔行保留原文。正文菜单复制完整 Markdown；代码块按钮与菜单复制 fence 内原始缩进 / 换行。HTTP(S) Markdown / 裸链接提供编号的打开 / 复制动作，保留配对括号与完整查询参数；不自动打开地址。助手回复上的分叉动作解析到同一 Run 的后继闭合边界，用户消息 / 未完成回合 / 断线时给出可见原因。鼠标、菜单键盘与 AX 共用动作序列，菜单使用实际滚动布局并在首帧布局后同步 AX。无 wire / schema / 生产依赖变化。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，221 passed / 0 failed；同参数 `cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。新增一个 GPUI 主路径回归，验证正文 / 代码精确复制、实际菜单 AX、同 Run 边界和断线禁用；表格、链接括号与流式解析断言并入原有两个 Markdown 测试。复用临时 rustc 索引 wrapper / test runner，单 Cargo 进程、无 clean。
- **代理真窗口已通过部分**：真实历史数据库的隔离副本重开「视觉验收」，表格与代码正常显示；正文复制无作者 / 时间，代码保留缩进与末尾换行；菜单 AX 动作可见且可执行；从回复分叉后只读 SQLite 核对新增 branch 指向 `evt-run-gui-1788924300692-1-872`（同 Run 终态），原始消息未改。100% / 125% / 150% 与拖窄窗口可阅读、滚动，Composer 未被覆盖。另一隔离任务的持久化用户样本经同一渲染器验证多行表格对齐、长链接换行 / 完整复制；打开后在默认浏览器地址栏核对完整 URL。样本文字中的“全部通过”等是历史模型输出，不是本批验收结论。字号恢复 100%，语言仍为中文。
- **仍待补验**：同一含表格 / 代码 / 长链接的新助手回复从流式到完成再重开的端到端链路。本批指定 `opencode-go / glm-5.3-flash / read-only` 的两次真实请求均为 `authentication / HTTP 401 / run_failed`；第二次使用既有验收凭据的隔离副本，未更换模型或改动原凭据。未用历史回复或用户样本冒充新请求成功。用户验收、归档均未完成。
- **候选与窗口外事实**：最终二进制 SHA-256 `99251d1ddfa1bba74ea21ac6c2bde49760fc4754be14d555099a0f8adc2e9c11`；`target/pawork-desktop-runtime/Pawork-UX02-verified.app` 与 `Pawork-UX02-links.app` 均为该 build 的独立 bundle。实际 argv 分别连接 `/tmp/pawork-ux02-replay/data/pawork-gui-replay.sock` 与 `/tmp/pawork-ux02/data/pawork-gui-ux02.sock`。数据库副本来自此前真实 `ui6b-quota-live` 验收记录；原库未修改。截图见本次工具记录，不检入仓库。
- **检查记录**：`/tmp/pawork-ux02-tests.log`、`/tmp/pawork-ux02-build.log`、`/tmp/pawork-ux02/candidate.json`、`/tmp/pawork-ux02/evidence.json`。审查发现的含括号 URL 截断与真窗口发现的菜单 AX 首帧同步已修复并复验。Rust 格式、文档本地链接、`git diff --check` 通过。未提交、推送或发布。


### UX-03 新任务与项目上下文引导

- **问题 / 证据**：“无项目”提示要求选择项目，但入口是只读文字；切换侧栏项目筛选后，当前对话仍无项目且从列表消失。初次用户难以区分“筛选列表”和“绑定任务”。报告 F03，截图 01 / 20 / 21。
- **最小任务**：明确项目筛选的作用；无项目提示提供“在项目中新建任务”入口，完成后带到新任务；筛选隐藏当前任务时明确说明。不得静默重绑旧会话。
- **输入区收紧（2026-09-09 用户截图补充）**：将“文件工具不可用”紧跟在项目状态右侧，与“在项目中新建任务”入口共用卡片下方的项目 / 上下文元信息行，普通宽度不再为项目限制另占一行。限制在无项目任务中直接可见；进入有项目的新任务后移除该提示，不把侧栏项目筛选当作当前任务已绑定项目。
- **验收**：从空态能完成添加 / 选择项目并创建有项目任务；新建前后项目归属可见；取消目录选择无副作用；现有无项目会话和草稿保留。
- **布局验收**：普通宽度下项目状态、限制说明与新建入口同排，右侧上下文读数可见；窄窗及三档字号下允许必要换行，限制说明与入口不被截断、遮挡；提示移除后回收所占高度，不保留空行。
- **范围**：Desktop TaskRail、空态、Composer 项目提示；复用已有 workspace / session 命令。状态：**已实现；定向自动检查通过；代理真窗口复验通过；用户验收待进行；未归档。**

#### UX-03 本批证据（2026-09-09）

- **实现**：侧栏入口标明「筛选 · 项目名」，当前任务被筛选隐藏时给出可见说明；筛选不改任务归属与草稿。空态和无项目任务的元信息行提供「在项目中新建任务…」，选择已有项目后创建并打开新任务；添加目录先等 Host 的 `workspace_add` 回执 / snapshot，再按返回项目创建。取消目录选择不派出项目 / 会话命令。无项目状态、文件工具限制与新建入口普通宽度同排，右侧保留上下文读数，窄窗允许换行；有项目后移除限制和入口，不留空行。AX 使用实际布局，鼠标 / 键盘 / AX 共用项目选项，Esc 回入口。无 wire / schema / 依赖变化。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，222 passed / 0 failed；`cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。新增一个 GPUI 主路径回归，覆盖筛选不重绑 / 不改草稿、项目菜单选项、Esc 回焦与第二次 Enter 确认、宽窄窗 × 三档字号下实际元信息框不重叠 / 不越界、移除限制时保留其他反馈；复用既有 Composer 布局与草稿测试。沿用临时 rustc 索引 wrapper / test runner，单 Cargo 进程、无 clean。
- **代理真窗口已通过**：空态添加 `/tmp/pawork-ux03/project` 后自动进入有项目新任务；取消系统目录选择后保持空态；无项目任务切项目筛选后仍保留原草稿、无项目限制和明确隐藏说明；100% / 125% / 150% 与拖窄到最小宽度时，项目状态 / 限制 / 新建入口 / 右侧上下文均完整可见。真窗口发现的第二次 Enter 被触发器吞掉问题已修复，并在最终候选以 Enter 打开、↓ 选项目、Enter 新建复验；返回旧任务恢复原草稿，新任务草稿独立保留。有项目任务中限制与入口移除并回收高度。字号恢复 100%，语言仍为中文。
- **候选与窗口外事实**：`target/pawork-desktop-runtime/Pawork-UX03-final.app`，SHA-256 `16310083dbdf72e764881f9320939ada22a2f891661603d63554ad617bb6ab3d`，与最终 build 相同；实际 argv 连接 `/tmp/pawork-ux03/data/pawork-gui-ux03.sock`。全新隔离 Host 使用当次 `opencode-go / glm-5.3-flash / read-only` 参数；只读 SQLite 核对 3 个 session（2 个属于新建的 canonical 测试项目，1 个原无项目任务仍为 NULL）、0 Run、0 session event。未发送消息或请求模型。截图见本次工具记录，不检入仓库。
- **检查记录与边界**：`/tmp/pawork-ux03-tests.log`、`/tmp/pawork-ux03-build.log`、`/tmp/pawork-ux03/candidate.json`、`/tmp/pawork-ux03/evidence.json`。Rust 格式、文档本地链接与 `git diff --check` 通过。用户验收、归档未完成；未提交、推送或发布。

### UX-04 错误在发生处解释并提供下一步

- **问题 / 证据**：只读模式仍可点击终端 Start，拒绝后完整 `ProtocolError` 塞在 Composer 下方并截断，终端保留占位文案；无法连接时中央仍要求新建任务，模型显示“加载中”。报告 F04，截图 25 / 26 / 40 / 41。
- **最小任务**：终端就地显示“只读模式不能启动终端”及可行下一步，技术详情可展开；已知不可用时预先说明。连接失败使用专门空态、可读原因与诊断入口；区分未连接、正在重连、失败、无模型。
- **验收**：同样的只读拒绝和无效端点下，用户不依赖 tooltip / AX / 日志就能看懂原因；重试有反馈；错误不挤占无关输入区。不得自动降低审批或修改安全默认。
- **范围**：Desktop Terminal、连接状态、错误投影与文案。状态：已实现；定向自动检查与代理真窗口复验通过；等待用户验收；未归档。


**本批证据（2026-09-09）**

- **已实现**：Terminal 就地解释只读限制、断线、启动中与操作失败，技术详情可展开；读取到有效只读权限或收到当前连接的明确拒绝后禁用创建，运行中终端的 I/O 错误仍保留可操作状态。权限已加载时进入审批设置，未知时进入连接诊断；不自动更改审批。主区连接提示提供可读原因、重试次数与诊断入口，已有任务和草稿保留；模型入口区分连接中 / 重连中 / 未连接 / 失败 / 目录加载中 / 无模型。新增动作支持鼠标、键盘与实际布局同源 AX。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，223 passed / 0 failed；`cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。新增一个 GPUI 主路径回归，覆盖错误归属、草稿保留、恢复入口、模型状态、只读创建 gate 与 stale 权限；扩充既有终端 I/O 回归，确认失败保留运行态、成功清除错误。沿用临时 rustc 索引 wrapper / test runner，单 Cargo 进程、无 clean。
- **代理真窗口已通过**：只读预检解释并禁用启动，设置入口可达；无效端点显示缺少连接凭证的原因，重复失败有次数反馈，诊断可达。为复现权限查询未知时的真实拒绝，临时本地 socket relay 仅扣留权限查询响应，其余帧转发真实 read-only Host；点击启动收到 Host 的实际拒绝，最终候选就地显示原因、禁用重复创建，技术详情完整换行。扣留查询导致的既有设置超时反馈单独保留，终端错误未写入 Composer。最终候选检查 100% / 125% / 150%、Tab / Shift+Tab / Enter 展开收起与诊断导航；字号恢复 100%，语言仍为中文。同批较早候选还验证 Host 停止后断线、重启并重试恢复同一任务及完整中文草稿；后续只细化重试次数与权限未知拒绝文案 / gate，草稿链路未再修改。
- **候选与窗口外事实**：`target/pawork-desktop-runtime/Pawork-UX04-verified.app`，SHA-256 `c1e68198a6ea6d4160230b6870ab95bc465d176ec8417985139ab0d2eb50fbcd`，与最终 build 相同；最终 argv 连接 `/tmp/pawork-ux04/data/relay.sock`，冷失败窗口连接隔离无效端点。隔离 Host 使用当次 `opencode-go / glm-5.3-flash / read-only` 参数；只读 SQLite 核对 1 个测试 session、0 Run、0 session event，未调用 Provider，未启动交互 shell。截图见本次工具记录，不检入仓库。
- **检查记录与边界**：`/tmp/pawork-ux04-tests.log`、`/tmp/pawork-ux04-build.log`、`/tmp/pawork-ux04/candidate.json`、`/tmp/pawork-ux04/evidence.json` 与真实拒绝 `/tmp/pawork-ux04/relay.log`。Rust 格式、文档本地链接与 `git diff --check` 通过。没有修改 wire、依赖或安全默认；用户验收、归档未完成，未提交、推送或发布。

## 3. P2：查找效率与呈现质量

### UX-05 模型选择与管理效率

- **问题 / 证据**：Composer 菜单从列表头打开，当前 `glm-5.3-flash` 不在可见区域；20 模型管理弹层每屏仅约 4～5 行，无搜索；未连接供应商的 fallback 模型与可用模型混列。报告 F05，截图 03 / 09。
- **最小任务**：打开菜单定位当前项；支持按名称 / ID 筛选；明确连接状态与目录来源；完整模型名可读取，保留键盘高亮滚动和 Esc 回焦。
- **验收**：从 20 项目录快速找到指定模型；鼠标与键盘都可定位当前项；搜索无结果有恢复入口；fallback 清晰标注，不误称认证成功，也不硬编码隐藏模型。
- **范围**：Desktop Composer 模型菜单与 Providers 模型弹层。状态：**已实现；定向自动检查通过；代理真窗口部分通过；用户验收待进行；未归档。**

#### UX-05 本批证据（2026-09-09）

- **实现**：Composer / Providers 共用名称与 ID 搜索（不区分大小写）和清除入口；Composer 打开定位当前项，供应商分组与高亮项展示已有连接状态 / 目录来源，备用目录明确不代表认证成功，不硬编码隐藏模型。鼠标悬停 / 方向键共用高亮，完整名称与 ID 可读；AX 使用菜单与内部列表实际布局。管理弹层固定搜索 / 状态 / 完整目录计数 / 批量动作，列表独立滚动，520px 高度上限；长名可横向阅读，键盘聚焦的 Switch 滚入视口。全开 / 全关仍针对完整供应商目录，写 gate 和 Host 回执语义不变。搜索 Enter 不发送草稿；真窗口发现的同键重复投递重开菜单已修复，Esc 回入口。无 wire / schema / 生产依赖变化。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，223 passed / 0 failed；同参数 `cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。更新现有模型菜单与管理弹层回归，覆盖 20 项当前项定位、大小写 ID 搜索、空结果 / 清除、Enter 选择不发送草稿、键盘滚动与 AX、Esc 回焦；管理弹层三档字号下搜索、无结果、实际可见 Switch、完整目录批量动作与原有 gate 回归通过。没有新增测试数量或测试框架。单 Cargo 进程，复用增量缓存与临时 rustc 索引 wrapper / runner，无 clean。
- **代理真窗口已通过部分**：初始候选在真实目录中自动滚入 `opencode-go / glm-5.3-flash`；普通键入 `GLM-5.3` 过滤、无结果提示 / 清除恢复、↑/↓ 高亮与滚动、Esc 回焦均可操作；管理弹层真实显示 Go 的 20 项目录与完整名称 / ID。最终候选复验 Enter 打开不立即关闭、↓ + Enter 选择后保持关闭、当前项定位与 Esc 回焦。未发送消息，没有运行其他模型；用于检查选择的模型仅为本地 pending。
- **仍待补验**：最终候选的备用目录鼠标状态、管理搜索 / 开关键盘操作，以及两个菜单宽窄窗口 × 三档字号的完整像素复验。继续操作时 CUA 返回 `ScreenCaptureKit / SCStreamErrorDomain -3811`（音视频捕捉无法启动）；状态读取、重置会话后按候选路径重新连接仍失败，按失败熔断停止重试。自动布局断言不替代这些真窗口结论。用户验收、归档未完成。
- **候选与窗口外事实**：任务起点 `main / 3128ed96`，工作区干净。候选 `target/pawork-desktop-runtime/Pawork-UX05-final.app`，SHA-256 `467b0b3596e93f8d6140647cd55944cef36b4bb80b739811d0d867d3cc9ee780`，与该生产实现 build 相同；此后仅扩充现有测试断言与文档。实际 argv 连接 `/tmp/pawork-ux05/data/pawork-gui-ux05.sock`；隔离只读 Host 当次参数 `opencode-go / glm-5.3-flash / read-only`，现有凭据仅复制到隔离目录供目录状态读取。SQLite 只读核对 0 session、0 Run、0 session event。截图仅在任务工具记录，不检入仓库。
- **检查记录**：`/tmp/pawork-ux05-tests.log`、`/tmp/pawork-ux05-build.log`、`/tmp/pawork-ux05/candidate.json`、`/tmp/pawork-ux05/evidence.json`。一次只读审查发现的鼠标浏览状态问题已用供应商分组来源与悬停高亮修正。Rust 格式、文档本地链接、`git diff --check` 通过；未提交、推送或发布。

### UX-06 供应商与账号信息层级

- **问题 / 证据**：连接中的 Go 排在多个未连接供应商之后；默认角色占据首屏大部；账号区层次弱，重置显示“6995 分钟”，当前额度与底栏“配额不可用”难以理解。报告 F06，截图 07 / 08。
- **最小任务**：让已连接供应商更易定位；未接线角色明显标注不可用 / 尚未生效；账号、当前选择、三窗额度与操作分组；重置时间转为小时 / 天或本地日期，保留准确值和来源说明；统一说明底栏与账号额度的范围。
- **验收**：用户能直接识别当前账号、三窗已用 / 剩余含义、重置时间及下一请求生效范围；未知 / 过期读数明确区分；不能相加共享订阅额度。
- **范围**：Desktop Settings Providers 呈现；不重做 G1 / G2 后端。状态：**已实现；定向自动检查通过；代理真窗口部分通过；用户验收待进行；未归档。**

#### UX-06 本批证据（2026-09-09）

- **实现**：已连接供应商优先稳定排序，render / AX 共用次序；角色默认值移至供应商列表后，识图 / 搜索在选择入口与用途说明直接标注「尚未生效」，保留保存偏好能力。展开区先账号与三窗额度，再账号操作、添加账号、代理及模型管理。当前选择只影响该供应商后续请求，进行中请求保持原账号。已用 / 剩余各自取已有字段，缺失不推算；倒计时保留分钟精度并转为天 / 小时 / 分钟，显示来源、获取距今秒数、过期与重置待确认。底栏改为「任务额度 —」，账号页说明任务与订阅范围及共享额度不可相加。无 wire / schema / 生产依赖变化。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，224 passed / 0 failed；`cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。更新已有角色 / Provider AX 回归，覆盖连接优先、角色位置、未生效说明、实际滚动、三窗已用 / 剩余 / 来源、缺失与过期；新增一个分钟精度换算主路径，含 6995 分钟 → 4d 20h 35m。复用临时 rustc 索引 wrapper / runner，单 Cargo 进程、无 clean。
- **代理真窗口已通过部分**：最终候选已连接供应商排在未连接项前，账号标题、名称、当前选择、三窗不可用、账号动作、添加入口和订阅 / 任务范围说明可读；宽窗及拖窄窗口的 100% / 125% / 150% 像素复验通过。角色区移到列表后，窄窗三档字号下未生效说明完整换行；鼠标打开、Tab / Shift+Tab、Enter 打开与 Esc 回焦通过，未写角色选择。较早候选额外点击刷新，真实经历加载后回到不可用；最终候选相关刷新逻辑未变。已恢复中文 / 100%。截图只在任务工具记录，不检入仓库。
- **仍待补验**：隔离 Go 账号当次真实三窗查询均不可用，未以 fixture 冒充真实订阅。真实数值、重置倒计时与过期读数的完整像素验收待可用账号查询恢复；自动断言不能替代此结论。未新增 / 删除 / 切换真实凭证，真实双账号切换仍是第 4 节的覆盖缺口。用户验收、归档未完成。
- **候选与窗口外事实**：起点 `main / 4fecf92c`，工作区干净。`target/pawork-desktop-runtime/Pawork-UX06-final.app`，SHA-256 `9a1c7c54e9a38cedd4239e18bff038a7315c813077d1ec74fd916a8270f0b640`，与最终 build 相同；实际 argv 连接 `/tmp/pawork-ux06/data/pawork-gui-ux06.sock`。独立只读 Host 当次参数 `opencode-go / glm-5.3-flash / read-only`；沿用已有隔离凭证副本，未输出明文。SQLite 只读核对 0 session、0 Run、0 session event，没有发送消息或运行模型。
- **检查记录**：`/tmp/pawork-ux06-tests.log`、`/tmp/pawork-ux06-build.log`、`/tmp/pawork-ux06/candidate.json`、`/tmp/pawork-ux06/evidence.json`。Rust 格式、文档本地链接、`git diff --check` 通过；未提交、推送或发布。

### UX-07 工具与运行事实可核查

- **问题 / 证据**：空目录调用展开后只有工具名和“已完成”，没有路径 / 空结果说明；数据库已有 usage 事件，完成后底栏仍全部为未知；取消卡与页脚重复陈述。报告 F07，截图 23 / 24 / 30。
- **最小任务**：工具展开显示已有参数、结果与明确空态；完成回合显示能从权威事件恢复的用量；没有来源的指标继续如实缺省；减少重复终态与无信息的常驻状态。
- **验收**：空目录显示 `.` 与 0 项；用户可区分成功空结果和没有数据；实时 / 重开任务的同一 Run 事实一致；不同任务间不串用用量、差异或配额。
- **范围**：Timeline / Run 状态投影及呈现；必要时定位已有 client 投影，先核查现有字段，不默认扩 wire。状态：**已实现；定向自动检查通过；代理真窗口部分通过；用户验收待进行；未归档。**


#### UX-07 本批证据（2026-09-09）

- **实现**：工具展开共用参数 / 结果文本与 AX / 测高，参数按工具身份、序号恢复，最终结果替换流式片段；空串与缺失分开，目录权威 metadata 显示路径与 0 项。工具完成 / Run 终态后通过既有 SessionGet 回填，实时与历史重叠去重；完成先到仍能合并历史开始。本地用户消息由持久化行替换，迟到回执不再追加第二份。Run 页脚 / 底栏仅消费本轮持久化终态累计 usage，缺失未知、零值有效；切任务清理事实，活动 Run 不沿用上一轮读数。取消只留页脚，失败卡保留原因；Review changes 仅附着当前任务最近的 completed Run。复用 TimelineItem 现有字段，未新增 wire 类型、磁盘 schema 或生产依赖；同步 Desktop / Protocol Spec 与 GUI 设计。
- **自动检查通过**：`cargo test -p pawork-protocol -p pawork-desktop --offline --lib --bins --tests --features gpui/runtime_shaders`，396 passed / 0 failed（Desktop 224、Protocol 172）；`cargo build -p pawork -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 成功。真窗口发现回填重复消息后，修正本地回显与取消卡测高条件，并再次通过 Desktop 224 项测试与 build。新增一个 protocol golden 主路径覆盖参数 / 空目录、重复和迟到、零值 / 缺失、基线清理；其余扩充现有用量、工具文本、本地回显与 AX 布局回归。复用临时 rustc 索引 wrapper / runner，单 Cargo 进程，无 clean。
- **代理真窗口已通过部分**：最终候选真实请求失败后只有一份用户消息，HTTP 401 原因直接显示，页脚不重复失败标题；底栏与页脚缺失用量均显示 `本轮用量 —`。切到新任务后时间线为空，再回旧任务恢复两轮各自失败事实；重开与实时一致。失败卡 / 缺值状态检查 100% / 125% / 150%，恢复中文 / 100%。截图仅在本次工具记录，不检入仓库。
- **仍待补验**：隔离 Host 固定 `opencode-go / glm-5.3-flash / read-only`，两次真实请求均 HTTP 401，没有实际工具完成或 usage；未改用其他模型。成功空目录展开、非零用量实时 / 重开一致性、取消页脚，以及这些内容宽窄窗 / 三档字号 / 键盘的完整像素验收待认证恢复；自动 golden 不替代真窗口证据。用户验收、归档未完成。
- **候选与窗口外事实**：起点 `main / e70c40c7`，工作区干净。最终候选 `target/pawork-desktop-runtime/Pawork-UX07-final.app`，SHA-256 `8fa59601a56098b3dbeb00fb05b10b39e0eb1912c36b49925cc9da4e9cc35b9c`，与最终 build 相同（此后仅格式与文档整理）；实际 argv 连接 `/tmp/pawork-ux07/data/pawork-gui-ux07.sock`。SQLite 只读核对 2 session、2 failed Run、10 持久化事件，终态均为 authentication / HTTP 401 且无 usage；测试目录仍为空。隔离凭据沿用已有副本，未修改真实凭证或持久默认。
- **检查记录**：`/tmp/pawork-ux07-tests.log`、`/tmp/pawork-ux07-build.log`、`/tmp/pawork-ux07-final-tests.log`、`/tmp/pawork-ux07-final-build.log`、`/tmp/pawork-ux07/final-candidate.json`、`/tmp/pawork-ux07/evidence.json`。独立代理调用被后端路由拒绝，主代理直接自查，不计作独立审查通过。Rust 格式、本地文档链接与 `git diff --check` 通过；未提交、推送或发布。

### UX-08 归档的可恢复性

- **问题 / 证据**：归档后任务立刻消失，界面回空态，没有撤销反馈或已归档入口；DB 只是 `archived=1`。报告 F08，截图 31。
- **最小任务**：提供归档完成反馈、撤销或明确恢复入口；保持归档和永久删除的语义区别。
- **验收**：误点归档能从 GUI 找回任务、正文与项目归属；重复操作不产生副本；仅针对本地隔离测试任务复验。
- **范围**：TaskRail 与已有会话归档命令；已确认 `archived:false` 可用，无需协议扩展。状态：**已实现；定向自动检查通过；代理真窗口复验通过；用户验收待进行；未归档。**

#### UX-08 本批证据（2026-09-10）

- **实现**：侧栏就地显示归档结果、正文保留说明与「撤销归档」；撤销记录明确保留到当前窗口关闭，支持连续归档后逐项撤销。复用既有 `session_archive` 的 `archived:false`，校验写后 Data 的任务 ID / 归档值；刷新失败不推翻已确认写入。在途防重复，断线禁写，失败 / 未确认请求保留恢复 ID。成功后打开原任务，解除可能隐藏它的项目筛选，恢复正文、项目归属与独立草稿；无新会话副本。鼠标 / 键盘 / 实际布局 AX 共用动作。无 wire / schema / 生产依赖变化。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，225 passed / 0 failed；`cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。新增一个主路径回归，覆盖身份 / 项目 / 草稿保留、重复调用、失败保留入口、断线 gate 与重试；复用既有归档 snapshot / 草稿 / 终端回归。沿用临时 rustc 索引 wrapper / runner，单 Cargo 进程，无 clean。
- **代理真窗口通过**：仅对 UX-07 隔离数据库的备份操作；含历史正文的项目任务归档后显示完成反馈，鼠标与 Tab + Enter 撤销均重新打开同一任务并恢复正文 / 项目 / 未发送中文草稿。连续归档项目任务和无项目任务后，依次恢复两者，各自草稿未串用；宽窗和拖窄窗口的 100% / 125% / 150% 下说明与按钮可见。字号恢复 100%，语言仍为中文。测试中误入改名后 Esc 取消，数据库标题未变。截图仅在本次工具记录，不检入仓库。
- **候选与窗口外事实**：起点 `main / dca9895b`，已有 auth / Mock 未提交改动，本批保留。候选 `target/pawork-desktop-runtime/Pawork-UX08.app`，SHA-256 `b2fa8f15fc20d8734aed406f3755f6b2f3e40d87487b0e1b76f39db2a1182c9f`；实际 argv 连接 `/tmp/pawork-ux08/data/pawork-gui-ux08.sock`。复用已有 UX-07 CLI Host 二进制，使用独立 data 目录与当次 `opencode-go / glm-5.3-flash / read-only` 参数；没有新 Run 或 Provider 请求。SQLite 只读核对归档中 `archived=1`，全部撤销后两会话的 ID / 标题 / archived / workspace_id 与验收前相同，仍为 2 个既有失败 Run、10 个既有事件，事件内容 SHA-256 前后一致；原数据库未改动。
- **检查记录与边界**：`/tmp/pawork-ux08-tests.log`、`/tmp/pawork-ux08-build.log`、`/tmp/pawork-ux08/candidate.json`、`/tmp/pawork-ux08/before.json`、`/tmp/pawork-ux08/evidence.json`。独立代理被后端加密路由拒绝，主代理直接自查，不计作独立审查通过。Rust 格式、文档本地链接与 diff 检查通过；用户验收、归档未完成，未提交、推送或发布。

### UX-09 视觉、文案与可访问性一致性

- **问题 / 证据**：中文界面混用 You、New session、Files、Summary、Start、tool completed 和 Host / ADR 实现术语；工具页无 MCP 时解释不存在的 Test / Remove；小字号元信息密集，缩小窗口后侧栏标题及长模型名较早截断。报告 F09，截图 10～17 / 22～27 / 34～39。
- **最小任务**：统一用户术语与中英文覆盖；让 MCP 空态给出真实配置路径；优化正文、辅助信息和控件的字阶 / 留白，减少重复描边和冗余状态；完成控件的实际命中框及焦点核查。
- **字号反馈收起（2026-09-09 用户截图补充）**：调整字号后的“字号 · 100% / 125% / 150%”仅作短暂反馈，随后自动消失并回收行高；当前值继续在外观设置中查看，不长期占据 Composer 下方空间。项目限制提示的合行归 UX-03；字号反馈的收起不得连带清除项目限制或尚需处理的错误。
- **验收**：初始宽窗与缩小窗口的三档字号下，关键标签和动作可见或可滚到；Tab / Shift+Tab / Esc 操作后焦点清楚；旁白可辨识名称与状态；不把本次 AX 自动化失败直接当作产品缺陷。
- **反馈验收**：连续调整字号时显示最后一次结果，短暂反馈结束后无残留空行，外观设置仍显示当前档位；其间出现的新反馈或错误不被旧字号反馈的收起误清除。
- **范围**：相应 Desktop i18n、布局、AX，沿用现有主题。状态：**已实现；定向自动检查通过；代理真窗口部分通过；用户验收待进行；未归档。**

#### UX-09 本批证据（2026-09-10）

- **实现**：回复作者、工具组、变更摘要、终端动作与对应 AX 补齐中英文；中文产品说明统一使用 Pawork 服务 / 项目，保留命令与原始内容。`New session` 保留占位标题仅显示为「新任务 / New task」，不写回。MCP 空态显示平台全局配置位置、配置节与重启刷新步骤，有服务器后再解释测试 / 移除。字号反馈单独保存，最后一次成功调整后 3 秒收起并回收行高，不清除错误或项目限制；外观页保留当前档位。终端目录 / 项目 / 状态移入可滚动正文，输入与启动 / 关闭的 AX 框使用实际布局。无 wire / schema / 生产依赖变化。
- **自动检查通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`，226 passed / 0 failed；`cargo build -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders` 成功。新增一个字号反馈主路径回归，覆盖连续调整、旧计时取消、新错误保留与行高回收；复用既有 i18n / 错误恢复回归检查双语词条与终端实际 AX 框。沿用临时 rustc 索引 wrapper / runner，单 Cargo 进程，无 clean。
- **代理真窗口已通过部分**：最终候选真实连接隔离 Host，中文任务标题、回复作者、消息动作和模型控件名称与可见界面一致；当前窄窗下时间线与 Composer 可见，模型入口焦点环清楚，字号反馈没有常驻空行。截图仅在本次工具记录，不检入仓库。已有工具记录无法完整恢复的操作不计入通过结论。
- **仍待补验**：MCP 空态、连续字号反馈与新错误共存、终端布局、长标题 / 长模型名的宽窄窗三档字号完整像素矩阵，以及 Tab / Shift+Tab / Esc 与系统旁白操作。本批使用隔离库的既有失败 Run，未验证成功工具内容、正常终端启动 / 停止。自动检查不替代这些真窗口证据；用户验收、归档未完成。
- **候选与窗口外事实**：本批保留已有 auth / Mock 工作，其间主干推进至 `f775febd`。最终候选 `target/pawork-desktop-runtime/Pawork-UX09-final.app`，SHA-256 `8a5ea488e39108e35e4e18845e194d9c225cc2bc69fedda8d3fd23774bc7fa95`，与最终 build 相同；实际 argv 连接 `/tmp/pawork-ux08/data/pawork-gui-ux08.sock`。复用隔离只读 Host 的当次 `opencode-go / glm-5.3-flash` 参数，没有新 Run 或模型请求；SQLite 只读核对仍为 2 个原会话、2 个既有失败 Run、10 个既有事件，数据库标题仍为原文。
- **检查记录与边界**：`/tmp/pawork-ux09-final-tests.log`、`/tmp/pawork-ux09-final-build.log`、`/tmp/pawork-ux09/final-candidate.json`、`/tmp/pawork-ux09/evidence.json`。独立代理被后端加密路由拒绝，主代理直接自查，不计作独立审查通过。Rust 格式、文档本地链接与 diff 检查通过；未提交、推送或发布。

<a id="ux-09-真窗口补验2026-09-10main--14ac19d4"></a>
#### UX-09 真窗口补验（2026-09-10，`main / 14ac19d4`）

- **本轮范围**：复用 UX-09 最终候选与 UX-08 隔离只读 Host；源码未改。本轮只补实际可操作的窗口证据，仍为**代理真窗口部分通过；用户验收待进行；未归档**。
- **MCP 空态通过**：初始窗口与拖窄窗口均检查 100% / 125% / 150%；全局配置路径、配置节、重启后刷新步骤完整可读，长说明自然换行；没有显示不存在的 Test / Remove 操作。
- **字号反馈通过**：连续按 Cmd+0 / Cmd+= / Cmd+= 只显示最后一次 150%；反馈消失后 Composer 下方行高回收，外观页仍显示当前档位。无项目任务的文件工具限制持续保留；字号计时期间触发 Cmd+Alt+N 的「没有需要关注的任务」后，旧计时只清除字号反馈，新反馈继续可见。此项是真实普通反馈共存，**不扩张为新错误共存已验**。
- **标题、模型与键盘已覆盖部分**：仅将隔离无项目任务临时改成长中文标题，检查拖窄窗口与放大窗口的三档字号；重命名、归档及页首动作仍可见，长标题按现有规则截断，不宣称全标题始终显示。模型搜索真实目录的 `deepseek-v4-flash-vision-exp`，按产品支持的最小窗口与可放大窗口核对 100% / 125% / 150%：100% 单行显示完整 ID，125% 在菜单宽度内可读，150% 换行后读全名称与完整 ID；上下键浏览会滚动，Esc 关闭菜单，Tab / Shift+Tab 的焦点框可见。没有选择其他模型或发起 Run。
- **终端已覆盖部分**：初始窗口 150% 与放大窗口三档字号下，只读模式的项目 / 目录、原因、审批设置入口、底部输入和禁用启动按钮可见；拖窄窗口后 Inspector 按现有宽度规则折叠。**没有启动终端、执行命令或改变权限**，正常终端生命周期仍待验。
- **窗口外核对与恢复**：候选 SHA-256 与 `target/debug/pawork-desktop` 均为 `8a5ea488e39108e35e4e18845e194d9c225cc2bc69fedda8d3fd23774bc7fa95`；实际进程仍连接 `/tmp/pawork-ux08/data/pawork-gui-ux08.sock`。长模型矩阵开始前原隔离 Host 已退出、候选显示 connection refused，随后以同一二进制路径和当次 `opencode-go / glm-5.3-flash / read-only` 参数恢复，再重连继续验收。恢复原任务标题 `New session`、原先查看的项目任务及中文 / 100%；SQLite 只读核对 2 个会话的 ID / 标题 / archived / workspace_id 与原记录一致，2 个 Run 和 10 个事件的全行内容哈希前后不变。改名更新了该隔离任务的更新时间，因此不声称整个 sessions 表未变化。记录见 `/tmp/pawork-ux09-supplement/evidence.json`，截图仅在本次工具记录，不检入仓库。
- **剩余缺口**：新错误与字号反馈共存、正常终端启动 / 输入 / 尺寸 / 退出、全局键盘与系统旁白，及成功工具内容的像素验收仍待补齐。未修改生产实现，不新增测试、不重复编译；本轮仅检查文档链接与 diff，既有 226 项测试 / build 保持为上一实现批次的记录。

## 4. 尚待补齐的验收证据

以下是覆盖缺口，不是已经确认的功能缺陷，也不应自动转成新增实现：

| 场景 | 本次边界 | 补验要求 |
| --- | --- | --- |
| 写入审批与非空 Changes | 当前隔离 Host 为 read-only，项目为空 | 用临时项目与明确受控权限检查审批卡、拒绝 / 一次允许、真实文件变更与 diff |
| 正常终端 | 本次验证了 read-only 拒绝，未启动交互 shell | 在允许终端的隔离 Host 检查输入、输出、尺寸与退出 |
| 真实 OAuth / 双账号额度切换 | 仅已有 Go 账号读取与运行；未新增或删除真实凭证 | 可用账号条件具备时检查登录返回、错误保旧、跨账号切换；不得用 fixture 冒充真实订阅 |
| 非空 MCP | 当前无配置，仅检查空态 | 使用指定测试 server 检查连接、资源、Test / Remove 反馈 |
| 完整键盘与旁白 / IME | UX-04 已补热断线重试与草稿恢复、新增动作 Tab / Shift+Tab / Enter；全局键盘、旁白与系统 IME 未完整覆盖 | 对修复后的候选补齐对应真实操作，不以源码或 AX 树代替像素验收 |

## 5. 初始验收基线与当前验证状态

41 张本次窗口证据、两次真实 completed Run、一次 cancelled Run，已与隔离 SQLite 事件和空目录事实核对；完整记录与证据索引见 [验收报告](gui-ux-audit-2026-09-09.md)。初始验收只修改文档与引用；随后 UX-01 / UX-02 / UX-03 / UX-04 / UX-05 / UX-06 / UX-07 / UX-08 / UX-09 的实现与验证记录见上方本批证据。没有提交、推送或发布。

Validated: 本轮 UX-09 真窗口补验覆盖 MCP 空态、字号反馈收起 / 新普通反馈保留、长标题动作、最小与放大窗口三档长模型搜索、局部键盘与只读终端布局；候选 SHA-256 / 实际 argv、SQLite 会话身份与 Run / 事件哈希、文档链接 / diff 检查。源码未改，未重跑 Cargo；Desktop 226 项测试与 build 属于上一实现批次，UX-01～08 既有记录见各项。

Targeted regressions: 本轮真窗口连续字号调整、行高回收、项目限制及计时期间新普通反馈保留；未新增自动测试。上一批自动回归含计时取消、新错误保留、双语词条与终端实际命中框。IME、流式、成功工具与非零用量等覆盖缺口保持待验。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
