# CU-13 · 内置浏览器截图与 DOM 观测

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS（P4）。前置：[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

真窗核对（2026-10-10，macOS，Pawork-cu13b + `ui_fixture serve --profile r6-terminal`，`PAWORK_UI_GUI_APPROVAL=1`，页面 `http://127.0.0.1:8937/index.html`）：审批卡「允许本次运行」放行后，browser 链为 navigate → read → screenshot → `#pw` / `#file` type 拒绝 → 再 read。截图为洋红底、绿条 `CU13-PAGE-MARKER`、密码框、文件框与文本框，与 fixture 页一致；Base64 解码后是 1024×768 JPEG（证据 `/tmp/cu13b/logs/browser-screenshot.b64`、`/tmp/cu13-20261009/screenshot.jpg`，不入库）。两次 type 回执均为「不支持密码或文件输入 / Password and file inputs are not allowed」。HTTP 日志对成功 run 各有一次 `GET /index.html`。窗口始终停在时间线，没有切到浏览器面板；Composer 连续性按下一段补证记录。

Composer 焦点连续性补证（同日，证据 `/tmp/cu13-focus/`）：`run-gui-1791584164805-4` 于 1791584165096 开始、1791584185126 完成；首个 navigate 工具 `browser-tool-96640-1791582916775963000-19` 的执行区间为 1791584166241–1791584180146。审批点击发生在 1791584165597，四组真实按键 `pr`、`e-`、`fl`、`ight` 的时间分别为 1791584166834–6846、7714–7724、8586–8600、9457–9480，全部落在 navigate 执行窗口内；HTTP GET `/index.html` 时间 1791584168095 也在同一窗口。每组按键后的新 AX 均显示 browser 工具 running、Cancel run 可用、run 时长从 00:01 递增到 00:05、焦点元素始终为 `composer-input`，Composer 值依次为 `pr`、`pre-`、`pre-fl`、`pre-flight`；同步截图与 AX 存于同目录。初审曾因旧 `in-flight.png` 为 run 完成态返回 P2，本段为同 reviewer 要求的补证结果，前两次未能采到进行中状态的尝试不作为结论。

独立 review（2026-10-10）：执行者为同线程 `xai/grok-4.7` / xhigh。R1c 受审快照 `/tmp/pawork-cu13-browser-observation-r1c.diff`，25434 字节，SHA256 `b3fb3a98302a1f4611a912c9ba0afb6b222eb0ea30bba3a7e0243c625791e964`；指挥在 PASS 前核对当前 diff 与快照 `cmp` 一致。唯一 reviewer 线程 `01a122ac-7ed6-76b2-af9b-7ca7ef4458cd`（`gpt-6.1-sol` / max）初审 turn `01a122ac-829e-7ed3-9caa-356c072803c5` 对焦点连续性返回 P2；补证后同 reviewer 复审 turn `01a122c0-a52a-7182-aa14-7687dd8a145e` 最终 `VERDICT: PASS`，确认四组按键位于 navigate 执行区间、AX/截图/HTTP 证据一致且无新增过度声明。reviewer 按要求只读，不运行 Cargo。

句柄 TTL 核对（同日同 Desktop/Host）：先跑新的 `fixture:browser`，从最后一次 read 回执取未消费的 `#echo` 文本句柄 `e-82585-1791580734407382000-126`（read 完成时间 1791582094523），等待 219287ms 后在真实 Desktop 输入 `fixture:browser-ttl <handle>`。type 执行开始于 1791582313810，工具回执 `is_error=true`：「元素句柄已过期，请重新读取 / Element handle expired — read the page again」；UI 停在 `fixture ttl rejected`，HTTP 与 serve 日志在 type 前后无新增，证明这不是页面重载、Host 重启或权限拒绝。证据在 `/tmp/cu13-ttl/`（timing、两次工具参数/回执、AX 快照、页面截图与日志，不入库）。

本轮只在 dev-only `crates/app/examples/ui_fixture.rs` 扩展模拟验收资产：`fixture-model` 声明 `image_input` 与 `tool_calls` 并经正式 `switch_model` 刷新目录；`PAWORK_UI_GUI_APPROVAL=1` 时把同一 `GuiApprovalHost` 装进 Core，使审批卡走正式 GUI；`fixture:browser` / `fixture:browser-ttl` 按前缀驱动真实 BrowserTool 链；工具调用 ID 使用进程级命名空间（pid + 启动纳秒）加进程内计数，避免多步工具或同一 session.db 重启 Host 后撞 `tool_calls` 全库主键；首张截图 Base64 与轨迹只写隔离 root 的 logs。实际检查：`RUSTC_WRAPPER= cargo build -p pawork-app --example ui_fixture --features ui-fixture` EXIT 0；`RUSTC_WRAPPER= cargo build -p pawork-desktop --features gpui/runtime_shaders` EXIT 0；`cargo test -p pawork-browser --lib observe::tests::handle_lifecycle_consumes_once_and_rejects_stale -- --exact` EXIT 0；修复命名空间后 `cargo build -p pawork-app --example ui_fixture --features ui-fixture` EXIT 0。同一 `/tmp/cu13b` session.db 停止旧 Host、重启新 Host 并重连 Desktop 后，完整 browser 链产生 `browser-tool-96640-1791582916775963000-1..6`，与既有 `browser-tool-1..85` 无冲突，HTTP 新增一次 `GET /index.html`（证据 `/tmp/cu13-toolid/`）。未把刚好 60/61 秒、click 句柄、已使用优先于过期、密码/文件句柄作为本轮真窗结论；这些边界仍由既有单测或后续任务覆盖。历史失败与恢复保留：一次 kache `rustc` 挂起被终止（EXIT 143）后按仓库允许的单次 `RUSTC_WRAPPER=` 绕过恢复；早期 DenyAll 立即拒绝、重复 `mock-tool-call-0` 主键冲突与裸计数 Host 重启撞全库主键分别定位为 fixture 审批装配和工具 ID 缺口，修复后由上述真窗链路复验。

## 目标

在现有系统 WebView 上补截图与稳定 DOM 观测，保持用户可同时使用 Pawork。

## 当前依据与读取范围

现有 WebKit 已支持有界 read/click/type、带代际和 DOM revision 的一次性元素句柄及真实 JPEG 截图；BrowserView 位于 AppKit 主线程，不能 Send / Sync。本轮核对的是既有能力的截图与输入连续性验收缺口。

[browser Spec](../spec/crates/browser.md)、[app Spec](../spec/crates/app.md)、[desktop crate Spec](../spec/crates/desktop.md)、[protocol Spec](../spec/crates/protocol.md)。

## 实施范围

- 复用 Host → GUI 请求/回执桥，截图、DOM 查找、元素动作返回目标/页面代际及新观测。
- 固定受限 DOM 脚本运行在系统网页内容进程，不引入 Core JS Runtime 或网页到工具的桥。
- 页面导航、重载、DOM 变化使旧句柄失效；保持唯一可见匹配、密码/文件输入拒绝和输出预算。
- 在隐藏/切任务及用户操作 Composer 时验证截图与 DOM 操作不抢系统焦点；不能以 AX 状态正常代替实际输入验证。
- 涉及浏览器 wire 形状时按 CU-02 的已裁决契约先做 golden/版本门控，再接操作实现。

## 完成条件与验证

- 真实 WebView 的截图和 DOM 结果对应同一目标页面，用户输入连续不受影响。
- 旧页句柄、歧义、越权、拒绝和超时均有明确结果，历史重放不重新执行网页动作。
- 页面与 HTTP 请求/DOM 事实联合验证，不能只用浏览器脚本自测证明产品通过。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 实现与历史验收记录（2026-10-07）

以下为实现轮记录，其中「未提交」描述当时工作区；当前基线已包含浏览器主体与复审修复。本轮状态以顶部及后续模拟验收记录为准，历史 AX / 模型配置阻塞不能代替当前环境核实。

实现（2026-10-07，主体已提交 `30b62e9e`，写入集 browser / app / desktop 三包 + browser.md / app.md / desktop.md / protocol.md 四篇 Spec；protocol wire 零改动，复用 GUI 1.18 请求/回执）：pawork-browser 新增页面观测与句柄——`read_page` 返回有界 JSON（页面文本压缩 + links / inputs / buttons 各带 CSS 选择器、视口 rect 与一次性元素句柄，输出 ≤64KiB 先压 text 再弹最长列表），附 `generation`（provisional 导航与内容进程终止 +1）、`dom_revision`（MutationObserver 计数，随机 key `__paworkObs_<nanos+seq>`）与 viewport；句柄 `e-<pid>-<boot_ns>-<seq>`、TTL 60s、一次性、cap 512 FIFO；`click_handle` / `type_handle` 先过新鲜度闸（观测 key 存在 + `location.href` 与签发时一致 + `dom_revision` 未变）再走既有唯一可见检查；`snapshot` 经 `takeSnapshotWithConfiguration` 在网页内容进程离屏渲染，不激活窗口、视图隐藏时可用，JPEG 最长边 ≤1280px、≤512KiB、质量阶梯 75/50/30。pawork-app 的 browser 工具扩展 screenshot / click_handle / type_handle 动作与校验，`browser_respond` 与工具输出上限 96KiB 提至 768KiB，click / type 成功后 Desktop 回一次新鲜观测（`observation` / `observation_error`）由 Host 透传，screenshot 走视觉门。apps/desktop 把浏览器派发后台化（`handle_controller_event` 即时派发、`raw_window_handle` 缓存、`cfg!(test)` 守门）并去掉强制打开面板。

实际命令与结果（2026-10-07，仓库统一 kache wrapper）：`bash scripts/test.sh browser app` 全绿（browser 11；app 281+2+1）；`bash scripts/test.sh desktop` 全绿（260 passed）。真实验收后发现视觉门缺陷并修复（未提交工作区 diff，见下），补跑 `cargo build -p pawork` 通过、`cargo test -p pawork-app --offline browser` 8 项全绿（含新增回归 `run_start_browser_screenshot_gate_falls_back_to_static_default`，红证：stash 修复后同测试零派发超时失败 EXIT 101）。

真实 / 人工结果（2026-10-07，macOS 26.6.2 arm64 本机）：平行 bundle `target/pawork-desktop-runtime/Pawork-cu13.app` + Host（`--instance cu13 --trust-workspaces --approval-mode never-ask --provider opencode-go --model glm-5.3-flash gui serve`）+ fixture HTTP 服务（127.0.0.1:8931，`/tmp/cu13/site/{index,page2}.html`，日志 `/tmp/cu13/http.log`）。未绑定项目的会话 8 次 browser 调用全部被 untrusted workspace 拒绝（fail-closed 正确）。绑定项目（ws-default）后 run 成功：HTTP 日志恰好 GET /index.html 与 GET /page2.html 各一次；read 返回真实观测 JSON（generation=1、dom_revision、viewport 1024×768、links / inputs / buttons 各带 selector + rect + handle）；句柄 click 返回 `clicked:true` 且动作后观测为 page2、generation=2；已消费句柄重用被明确拒绝（Element handle already used——仅证明一次性拒绝，未覆盖 TTL 过期）；对 page2 上不存在元素的选择器 type 被拒绝（No visible matching element——页面无密码框，未走到密码拒绝路径）；Desktop 退出重启、重连重放后 HTTP 日志仍 2 条、动作零重执行。结果 JSON 全量在 `/tmp/cu13/results.jsonl`（临时证据，不入库）。真实发现并修复缺陷：截图被视觉门误拒——静态目录把 glm-5.3-flash 锚定在 glm-coding，`run_start.rs` 的 provider 过滤丢证据后无回退；修复为 `browser_image_input` 计算补 `.or_else(default_image_input)`（与 `services/run.rs` text 门同口径），新增定向回归钉住。临时配置改动（opencode-go disabled_models 移除 glm-5.3-flash）验收后已恢复（与备份逐字节一致）；Desktop / Host / fixture 服务已停止。

遗留缺口：截图正路径真实闭环（Desktop 发起的 run → screenshot → JPEG 内容与目标页对应）与焦点连续性真实按键探针（browser 动作执行期间向 Composer 打真实按键、验证落字且前台不变）均因 macOS 26 AX 系统性失效未执行——AX 树呈嵌套 AXApplication、无 AXWindow，System Events 报 0 windows，`ui-ax-dump` / `ui-focus-switch` 不可用，且不应抢占正在使用机器的用户焦点（证据 `/tmp/cu13/ax5-7.txt`、ax8）；隐藏执行已有旁证（run 期间窗口显示 timeline 而非浏览器面板、浏览器标签保持关闭）。同批待真实核对的拒绝路径：真实密码 / 文件输入框的 type / click（`rejectSecret` 分支）与句柄 TTL 过期（区别于一次性消费）——fixture 页需补密码框与 >60s 等待才能覆盖，当前仅由脚本断言与单测锚定。以上待 AX 恢复或人工执行后验收；Host 侧视觉门修复已由定向回归锚定。`run_start.rs` 修复与新增回归为工作区未提交 diff（主体 `30b62e9e` 已由指挥方提交），提交时机由指挥方决定。

复审修复（2026-10-07，6.1 sol 审查 4 项必须修，全部落地，未提交）：① apps/desktop `ui/browser.rs` 派发改为绑定派发时的 `BrowserView`——后续导航等待、状态读取、脚本调用与动作后观测全部作用于绑定的原任务页面，不再跨 await 重读 `view.browser.native`；click/type 观测前复核归属（active session/run + `Rc::ptr_eq` 当前 native），切任务时保留动作事实并记 `observation_error`（"任务已切换，动作后观测跳过"）。② crates/browser `macos.rs` `snapshot` 与 `read_page` 的 completion 回调按共享 `Rc<Cell<u64>>` 核对页面代际：派发与回调之间发生导航 / 内容进程终止即明确拒绝（"页面在截图 / 读取期间导航"），不把新页像素或观测标成旧页；`finalize_page_json` 的 url/title 改为以同次脚本在页面读到的值为准，派发前 state 仅作缺失回退。③ 观测状态（revision 计数器）迁入隔离 `WKContentWorld`（`worldWithName:"pawork-observe"`，求值经 inFrame 形态 selector，见复审二）：页面主世界无法枚举或改写 `window[KEY]`，不能靠复原 revision 让旧句柄通过新鲜度闸；prologue 安装 MutationObserver 与状态存储任一失败时脚本显式拒绝（"观测状态安装失败，拒绝签发句柄"），不降级签发句柄。④ 本文档验收声明改为实际验证的拒绝类型（见上段）。定向回归：`finalize_page_json` / `finalize_observation` 脚本元数据优先（红证：改前断言失败）、`read_page_script` 安装失败显式拒绝（红证：改前断言失败）、state 回退路径正例。

复审修复二（2026-10-07，3 项必须修，全部落地，未提交）：① SDK 只声明 `evaluateJavaScript:inFrame:inContentWorld:completionHandler:`（WKWebView.h:291 核对），无 inFrame 的 selector 运行期不存在——补 `inFrame: nil`（主框架）。真实 WebView 验证：临时探针（activation policy Prohibited 不抢焦点、无窗口父视图）经本地 fixture 导航后在真实 WKWebView 上连续两次 `read_page` 成功（含 url / dom_revision / 句柄，状态跨次复用），输出 `/tmp/cu13/probe-selector.log`，探针用后即删。探针另暴露并修复 `state()` 的空指针：首次导航开始前 URL 为 nil，向 nil 发 `absoluteString` 触发 debug 断言，现按空地址处理。② desktop 派发起点错绑：浏览器归属只在 render / Inspector 切换时同步，切任务后、下一帧前收到的请求会绑定旧任务页面（close 同理可能关旧页）——派发闭包先 `sync_browser_session(cx)`，再核对 `browser_session` 与请求 session 一致才绑定。③ 观测状态发布时序：安装失败但存储成功会残留 `{revision:0}`，下一次 read 因已有数字 revision 跳过安装检查、签发无变化监测的句柄——状态发布（defineProperty / 直接赋值）收进 Observer 安装成功的分支，失败不残留状态、后续 read 重试安装并继续拒绝；新增红证回归 `read_script_publishes_state_only_after_observer_installs`（改前失败：发布未被 gate 包裹）。
