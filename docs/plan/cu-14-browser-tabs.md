# CU-14 · 内置浏览器标签页与任务生命周期

> 状态：自动回归通过，真实验收主体完成；复审修复四项已落地并过自动门禁（真窗复验与标签条滚动复验待 Mac 解锁）。前置：[CU-13](cu-13-browser-observation.md)、[CU-09](cu-09-ownership-cancellation.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

将每任务单页扩展为有归属的多标签，支持选择与清理。

## 当前依据与读取范围

当前每任务一个内存 WebView，新窗口链接在当前页打开；没有多标签或跨启动恢复。

[browser Spec](../spec/crates/browser.md)、[desktop crate Spec](../spec/crates/desktop.md)、[app Spec](../spec/crates/app.md)、[protocol Spec](../spec/crates/protocol.md)。

## 实施范围

- 在现有 browser / Desktop 消费面实现列出、创建、选择和关闭标签，绑定 session/run 与页面代际。
- 区分 Agent 创建的标签与用户已有标签；清理只作用于本任务拥有的目标，选择标签不自动前置系统窗口。
- 保留隔离非持久网站数据、隐藏/切任务保留和关闭释放的既有语义，不提前实现下载或跨启动 profile 管理。
- 新增 action 与 UI 先完成必要 wire golden/版本协商，不让旧 Host 接收未知命令。

## 完成条件与验证

- 多页操作归属准确，任务切换、取消、断线和关闭不影响其它任务或用户标签。
- 创建、选择和清理过程中用户键鼠/焦点不受影响，真实页面状态可核对。
- 旧 Host 明确禁用新能力，历史重放只展示，不再创建或关闭标签。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 复审修复（2026-10-07）

6.1 sol 复审结论 ISSUES，四项修复推翻或收紧了主体实现的两处口径：

1. **Agent 不再切换面板展示**。主体实现里 `new_tab` / `select_tab` 会把展示切到 Agent 标签并 `set_visible(false)` 隐藏用户当前页——macOS 上该调用对聚焦网页等同 `focus_parent()`，即打断用户输入，「双指针互不劫持」不成立。修复后 Agent 建 / 选标签只动 `agent` 派发指针，展示切换只能由用户触发；`select_tab` 回执的 `active` 改报真实值。
2. **地址栏标签事件锚点**。主体修复①只按 `url_changed` 更新地址栏且排除空 URL：新建空白标签 / 切到空白标签 / 关最后一张后地址栏残留旧 URL。修复为 `synced_tab` 锚点识别「切换标签」事件（允许空地址覆盖输入框），标签清零显式清空输入框与错误；错误归属在 `user_select` / `apply_close_plan` 换标签时清理——地址栏导航失败的错误与输入原样保留；browser-go 成功后钉住锚点，新建标签导航在途的空地址不会清掉刚输入的 URL。
3. **旧 Desktop 方言探测**。主体「旧 Host 明确禁用」口径不覆盖另一半：新 Host 认识 `close` / `navigate`，旧 Desktop 也认识它们但按旧语义执行（关用户标签 / 占用户页），对未知新动作的 fail-closed 保护不了已知动作。修复为 BrowserTool 每 run 首个动作先以 `tabs` 探测：回 `ok:false` + `Unsupported action`（旧 Desktop 有页面）或 `Browser has no page`（旧 Desktop 尚未创建页面；新 Desktop 的 tabs 是只读列举，零标签也成功，两种回执都只可能来自旧方言）即确认旧方言，本 run 禁用 browser 并提示升级 Desktop（探测失败如任务切换 / 超时不缓存、原样透传）；探测只付每 run 首个动作一次 RTT。复审二轮补：无页面回执最初未识别，反复提示先导航、而导航也先探测，形成无法恢复的重试循环——两种回执都入判别式。
4. **AX「+」按钮裁剪**。标签按钮按 frame 裁剪发布 AX，末尾「+」未裁剪，溢出时报告不可见按钮；修复为同一 `clip()`，零面积不发布。

复审修复验证（2026-10-07）：`bash scripts/test.sh browser app` 全绿（app 284，含新增 `browser_old_desktop_dialect_disables_run_without_dispatching`——探测确认旧方言后真实动作零派发、后续动作命中缓存零探测；既有 screenshot / chat_controls 测试改为先应答探测再应答真实动作）；`bash scripts/test.sh desktop` 262 全绿。复审①②④ 是 Desktop 真窗行为，GPUI 测试构造不了真实 BrowserView，真窗复验（用户在标签 1 时 Agent new_tab / navigate 不切展示、地址栏跟空标签清空）待 Mac 解锁后补做，与标签条滚动复验同批。

## 完成记录

实现（2026-10-07，写入集 browser / app / desktop 三包 + browser.md / app.md / desktop.md / protocol.md 四篇 Spec；protocol wire 零改动，复用 GUI 1.18 请求/回执，新动作是不透明 JSON 方言——旧 Host 的 schema enum 不含新动作即明确禁用，旧 Desktop 对未知动作 fail-closed 回 {ok:false}，历史重放仍只展示不执行）：pawork-browser 新增 `generation()` 暴露页面代际（unsupported 桩返回 0）。pawork-app 的 browser 工具 description / input_schema 新增 `tabs` / `new_tab` / `select_tab` / `close_tab` 与 `tab_id` 属性（`tab-` 前缀、≤64 字符），validate() 重构为 valid_url / valid_tab_id 闭包，语义保持 Host 不透明。apps/desktop 的 BrowserPanel 重构为 `tabs: Vec<BrowserTab{id, agent_owned, native}>` + 双指针：`active`（面板展示，用户视角）与 `agent`（Agent 派发目标）分离——用户切标签不劫持 Agent 派发，Agent 后台操作不抢焦点。归属规则：创建者即归属者；`navigate` 无 Agent 绑定时新建 Agent 标签（不占用户页）；`close_tab` 只关 Agent 标签（用户标签 / 未知标签明确拒绝）；`close` 只清理本任务 Agent 标签，清零后退回整面板关闭；用户 UI 可关任意标签。每面板标签上限 16；标签 id `tab-<pid>-<进程级序号>`。归属决策抽成纯函数（TabMeta / ClosePlan / plan_agent_close / plan_agent_close_all / plan_user_close / close_plan_for）：选中标签被关闭落到原位置下一张（末尾取前一张），Agent 绑定随其标签移除失效。标签条 UI：160px 截断标题按钮 + 24px 关闭钮 + Agent 色点与「· Agent」后缀 + 「+」新建，横向滚动（overflow_x_scroll），AX 发布 `browser-tabs` 组与 select / close 动作；地址栏 / 状态栏 / 检查器标题跟随 active 标签。

实际命令与结果（2026-10-07，仓库统一 kache wrapper）：`bash scripts/test.sh browser app` 全绿（app 283+2+1、browser 14，含新增 `browser_validate_accepts_tab_actions_and_rejects_malformed`）；`bash scripts/test.sh desktop` 三轮全绿（262 passed，含新增 `agent_close_own_tab_fixes_selection_and_agent_binding` / `agent_close_rejects_user_and_unknown_tabs`）。

真实结果（2026-10-07，macOS 26.6.2 arm64 本机）：平行 bundle `target/pawork-desktop-runtime/Pawork-cu14.app` + Host（`--instance cu14 --trust-workspaces --approval-mode never-ask --provider opencode-go --model glm-5.3-flash gui serve`）+ fixture HTTP 服务（127.0.0.1:8932，`/tmp/cu14/site/{index,page2,page3}.html`，请求日志 `/tmp/cu14/http.log`）。经 Codex 计算机操作驱动真实窗口（本轮 AX 可用，CU-13 的 System Events 阻塞不复现）。三次真实 Run 全部经审批卡「允许本次运行」：

- Run 1（navigate page2 → new_tab page3 → tabs → close_tab page3 → tabs → close → tabs）：用户标签 tab-46759-1（地址栏手动导航 index.html 创建）全程保留；tabs 正确报告归属（用户标签 agent_owned=false，两个 Agent 标签 true）；close_tab 关 page3 后 Agent 绑定失效（绑定随标签移除）；close 只回收 Agent 标签 tab-46759-2，面板保持打开，用户标签成为选中。HTTP 日志每页恰好一次 GET（page2 / page3 各一），无重放幻影请求。
- Run 2（navigate page3 → close_tab 用户标签 tab-47438-1 → close → tabs）：close_tab 用户标签被明确拒绝，错误原文 `Tab was opened by the user; the agent cannot close it / 标签由用户创建，Agent 不能关闭`；close 只回收 Agent 标签 tab-47438-3（`closed_tabs: ["tab-47438-3"]`）；tabs 剩余 2 个用户标签。
- Run 3（navigate page2 → new_tab page3 → select_tab 回 page2 标签 → read → close）：select_tab 切换派发目标后 read 命中 Page2（标题 / URL / #p2-input 输入框均真实），证明 read 跟随 Agent 绑定标签而非最近创建标签；close 回收两个 Agent 标签，用户标签不动。

跨进程事实核对：Desktop 退出重启后会话历史重放完整、HTTP 日志零新增（历史不重执行）；断线 Host 重启后 Desktop 手动重试重连、草稿保留。验收中发现并修复两处缺陷（未提交工作区 diff）：① 地址栏在 Agent close / close_tab 与用户切换标签后停留旧标签 URL——根因是 apply_close_plan / select_agent_tab / user_select 预写 self.state 使 sync_browser_state 的 url_changed 检测失效，修复为三处不预写、由随后 sync 检测变化更新输入框，close 分支补 sync；真窗复验通过（切标签与 close 后地址栏均与展示页一致）。② 3 个 160px 标签即溢出检查器宽度截断「+」与关闭钮，标签条改 `overflow_x_scroll`（沿用 subagent chips 同款模式）；截图证据均不落库。临时配置改动（opencode-go disabled_models 移除 glm-5.3-flash）验收后已恢复（与备份逐字节一致）；Desktop / Host / fixture 服务已停止。

遗留缺口：标签条横向滚动的真窗像素复验未执行——修复后重启验收窗口时 Mac 锁屏（自动解锁不可用），仅由 desktop 测试与既有 subagent chips 同款滚动模式锚定；桌面解锁后开 3+ 标签即可核对。其余 CU-13 登记缺口（截图正路径、焦点探针、密码 / 文件输入拒绝、句柄 TTL）不在本任务范围，继续在 CU-13 跟踪。
