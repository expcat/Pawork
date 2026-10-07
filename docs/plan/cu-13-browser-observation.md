# CU-13 · 内置浏览器截图与 DOM 观测

> 状态：已实现；自动回归通过，read/click/句柄/拒绝/重放真实验收通过，截图正路径与焦点探针待人工验收（P4）。前置：[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在现有系统 WebView 上补截图与稳定 DOM 观测，保持用户可同时使用 Pawork。

## 当前依据与读取范围

现有 WebKit 支持有界 read/click/type 等固定 DOM 操作，没有截图或观测元素句柄；BrowserView 位于 AppKit 主线程，不能 Send / Sync。

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

## 完成记录

实现（2026-10-07，主体已提交 `30b62e9e`，写入集 browser / app / desktop 三包 + browser.md / app.md / desktop.md / protocol.md 四篇 Spec；protocol wire 零改动，复用 GUI 1.18 请求/回执）：pawork-browser 新增页面观测与句柄——`read_page` 返回有界 JSON（页面文本压缩 + links / inputs / buttons 各带 CSS 选择器、视口 rect 与一次性元素句柄，输出 ≤64KiB 先压 text 再弹最长列表），附 `generation`（provisional 导航与内容进程终止 +1）、`dom_revision`（MutationObserver 计数，随机 key `__paworkObs_<nanos+seq>`）与 viewport；句柄 `e-<pid>-<boot_ns>-<seq>`、TTL 60s、一次性、cap 512 FIFO；`click_handle` / `type_handle` 先过新鲜度闸（观测 key 存在 + `location.href` 与签发时一致 + `dom_revision` 未变）再走既有唯一可见检查；`snapshot` 经 `takeSnapshotWithConfiguration` 在网页内容进程离屏渲染，不激活窗口、视图隐藏时可用，JPEG 最长边 ≤1280px、≤512KiB、质量阶梯 75/50/30。pawork-app 的 browser 工具扩展 screenshot / click_handle / type_handle 动作与校验，`browser_respond` 与工具输出上限 96KiB 提至 768KiB，click / type 成功后 Desktop 回一次新鲜观测（`observation` / `observation_error`）由 Host 透传，screenshot 走视觉门。apps/desktop 把浏览器派发后台化（`handle_controller_event` 即时派发、`raw_window_handle` 缓存、`cfg!(test)` 守门）并去掉强制打开面板。

实际命令与结果（2026-10-07，仓库统一 kache wrapper）：`bash scripts/test.sh browser app` 全绿（browser 11；app 281+2+1）；`bash scripts/test.sh desktop` 全绿（260 passed）。真实验收后发现视觉门缺陷并修复（未提交工作区 diff，见下），补跑 `cargo build -p pawork` 通过、`cargo test -p pawork-app --offline browser` 8 项全绿（含新增回归 `run_start_browser_screenshot_gate_falls_back_to_static_default`，红证：stash 修复后同测试零派发超时失败 EXIT 101）。

真实 / 人工结果（2026-10-07，macOS 26.6.2 arm64 本机）：平行 bundle `target/pawork-desktop-runtime/Pawork-cu13.app` + Host（`--instance cu13 --trust-workspaces --approval-mode never-ask --provider opencode-go --model glm-5.3-flash gui serve`）+ fixture HTTP 服务（127.0.0.1:8931，`/tmp/cu13/site/{index,page2}.html`，日志 `/tmp/cu13/http.log`）。未绑定项目的会话 8 次 browser 调用全部被 untrusted workspace 拒绝（fail-closed 正确）。绑定项目（ws-default）后 run 成功：HTTP 日志恰好 GET /index.html 与 GET /page2.html 各一次；read 返回真实观测 JSON（generation=1、dom_revision、viewport 1024×768、links / inputs / buttons 各带 selector + rect + handle）；句柄 click 返回 `clicked:true` 且动作后观测为 page2、generation=2；已消费句柄重用被明确拒绝（Element handle already used——仅证明一次性拒绝，未覆盖 TTL 过期）；对 page2 上不存在元素的选择器 type 被拒绝（No visible matching element——页面无密码框，未走到密码拒绝路径）；Desktop 退出重启、重连重放后 HTTP 日志仍 2 条、动作零重执行。结果 JSON 全量在 `/tmp/cu13/results.jsonl`（临时证据，不入库）。真实发现并修复缺陷：截图被视觉门误拒——静态目录把 glm-5.3-flash 锚定在 glm-coding，`run_start.rs` 的 provider 过滤丢证据后无回退；修复为 `browser_image_input` 计算补 `.or_else(default_image_input)`（与 `services/run.rs` text 门同口径），新增定向回归钉住。临时配置改动（opencode-go disabled_models 移除 glm-5.3-flash）验收后已恢复（与备份逐字节一致）；Desktop / Host / fixture 服务已停止。

遗留缺口：截图正路径真实闭环（Desktop 发起的 run → screenshot → JPEG 内容与目标页对应）与焦点连续性真实按键探针（browser 动作执行期间向 Composer 打真实按键、验证落字且前台不变）均因 macOS 26 AX 系统性失效未执行——AX 树呈嵌套 AXApplication、无 AXWindow，System Events 报 0 windows，`ui-ax-dump` / `ui-focus-switch` 不可用，且不应抢占正在使用机器的用户焦点（证据 `/tmp/cu13/ax5-7.txt`、ax8）；隐藏执行已有旁证（run 期间窗口显示 timeline 而非浏览器面板、浏览器标签保持关闭）。同批待真实核对的拒绝路径：真实密码 / 文件输入框的 type / click（`rejectSecret` 分支）与句柄 TTL 过期（区别于一次性消费）——fixture 页需补密码框与 >60s 等待才能覆盖，当前仅由脚本断言与单测锚定。以上待 AX 恢复或人工执行后验收；Host 侧视觉门修复已由定向回归锚定。`run_start.rs` 修复与新增回归为工作区未提交 diff（主体 `30b62e9e` 已由指挥方提交），提交时机由指挥方决定。

复审修复（2026-10-07，6.1 sol 审查 4 项必须修，全部落地，未提交）：① apps/desktop `ui/browser.rs` 派发改为绑定派发时的 `BrowserView`——后续导航等待、状态读取、脚本调用与动作后观测全部作用于绑定的原任务页面，不再跨 await 重读 `view.browser.native`；click/type 观测前复核归属（active session/run + `Rc::ptr_eq` 当前 native），切任务时保留动作事实并记 `observation_error`（"任务已切换，动作后观测跳过"）。② crates/browser `macos.rs` `snapshot` 与 `read_page` 的 completion 回调按共享 `Rc<Cell<u64>>` 核对页面代际：派发与回调之间发生导航 / 内容进程终止即明确拒绝（"页面在截图 / 读取期间导航"），不把新页像素或观测标成旧页；`finalize_page_json` 的 url/title 改为以同次脚本在页面读到的值为准，派发前 state 仅作缺失回退。③ 观测状态（revision 计数器）迁入隔离 `WKContentWorld`（`worldWithName:"pawork-observe"`，求值经 inFrame 形态 selector，见复审二）：页面主世界无法枚举或改写 `window[KEY]`，不能靠复原 revision 让旧句柄通过新鲜度闸；prologue 安装 MutationObserver 与状态存储任一失败时脚本显式拒绝（"观测状态安装失败，拒绝签发句柄"），不降级签发句柄。④ 本文档验收声明改为实际验证的拒绝类型（见上段）。定向回归：`finalize_page_json` / `finalize_observation` 脚本元数据优先（红证：改前断言失败）、`read_page_script` 安装失败显式拒绝（红证：改前断言失败）、state 回退路径正例。

复审修复二（2026-10-07，3 项必须修，全部落地，未提交）：① SDK 只声明 `evaluateJavaScript:inFrame:inContentWorld:completionHandler:`（WKWebView.h:291 核对），无 inFrame 的 selector 运行期不存在——补 `inFrame: nil`（主框架）。真实 WebView 验证：临时探针（activation policy Prohibited 不抢焦点、无窗口父视图）经本地 fixture 导航后在真实 WKWebView 上连续两次 `read_page` 成功（含 url / dom_revision / 句柄，状态跨次复用），输出 `/tmp/cu13/probe-selector.log`，探针用后即删。探针另暴露并修复 `state()` 的空指针：首次导航开始前 URL 为 nil，向 nil 发 `absoluteString` 触发 debug 断言，现按空地址处理。② desktop 派发起点错绑：浏览器归属只在 render / Inspector 切换时同步，切任务后、下一帧前收到的请求会绑定旧任务页面（close 同理可能关旧页）——派发闭包先 `sync_browser_session(cx)`，再核对 `browser_session` 与请求 session 一致才绑定。③ 观测状态发布时序：安装失败但存储成功会残留 `{revision:0}`，下一次 read 因已有数字 revision 跳过安装检查、签发无变化监测的句柄——状态发布（defineProperty / 直接赋值）收进 Observer 安装成功的分支，失败不残留状态、后续 read 重试安装并继续拒绝；新增红证回归 `read_script_publishes_state_only_after_observer_installs`（改前失败：发布未被 gate 包裹）。
