# pawork-browser

> 系统网页视图库。首版 macOS WebKit，供 Desktop 的任务浏览器使用；无 `pawork-*`、GPUI、Core 或协议依赖。

## 1. 职责与边界

承载手动导航及宿主授权 DOM 操作的原生视图、HTTP(S) 导航、历史、状态与生命周期。纯 Rust 调用系统 WebKit，不引入 Node / Bun / V8 或自有 JS Runtime；网页脚本由系统内容进程执行。每个视图使用隔离的非持久网站数据，不继承 Safari / Chrome profile。没有网页到 Rust 的 IPC、模型调用或自定义本地资源协议；自动操作只接受宿主调用，网页不能主动调用工具。

## 2. 模块与文件地图

| 路径 | 内容 |
| --- | --- |
| `src/lib.rs` | `BrowserState`、地址规范化、公开 facade、非 macOS 不支持分支与定向测试 |
| `src/external.rs` | 显式读取 Chrome/Edge 当前页的有界只读快照 |
| `src/ext/mod.rs` | 外部控制通道入口：`BrowserKind`、`default_data_dir()`（桥进程由浏览器拉起、无法继承 Desktop 路径，故自行解析：`PAWORK_DATA_DIR` 覆盖 → `$HOME/.pawork` → 临时目录；与 pawork-app / Desktop 的差异：无 Windows `LOCALAPPDATA`、空 `HOME` 视为缺失、无降级事件。桥为 `cfg(unix)`，Windows 分支当前无消费者；本包无内部依赖，不得改为依赖 app） |
| `src/ext/origin.rs` | 扩展 `location.origin` 归一化：`url::Url` 解析，小写 scheme/host、省略 80/443、丢弃路径 / 查询 / 片段，拒非 http(s)、userinfo、IPv6 与非法端口。与 computer-use CU-16 `approval` 的网站身份不同（后者保留显式端口并拒绝带路径输入），两者服务不同调用，不合并 |
| `src/ext/protocol.rs` | 帧编解码（u32 LE 长度前缀 + JSON，1 MiB 上限）、桥代际、`xt-` 句柄 wrap/unwrap、token 生成与测试 |
| `src/ext/discovery.rs` | 发现目录 `<data>/external-browser/` 的 `<browser>-<pid>.json` 原子读写（0600/0700）与扫描 |
| `src/ext/host.rs` | 桥进程服务器：stdio（扩展侧）+ unix socket（Desktop 侧）、token 鉴权、enabled 门、站点绑定、串行中继与 15s 保活 ping |
| `src/ext/client.rs` | Desktop 侧无状态客户端：逐动作扫描发现文件、hello 验 token、单启用规则、中继一个动作 |
| `src/bin/pawork_browser_bridge.rs` | 桥二进制入口：argv origin 校验（钉死扩展 ID）+ 父进程浏览器识别（macOS proc_pidpath） |
| `extension/` | 固定功能 MV3 扩展（manifest 钉死 key → ID `hgalnnlhankacinahkjmflinpobpanjh`）：sw.js 固定动作集、popup 启用开关、README 安装步骤 |
| `src/dom.rs` | 固定 DOM 脚本（含观测脚本与句柄动作脚本）、参数转义、唯一可见元素检查、JSON 输出预算与定向测试 |
| `src/observe.rs` | 元素句柄表（代际 / TTL / 一次性）与观测 JSON 收尾（代际、视口、句柄注入与预算） |
| `src/macos.rs` | WebKit / AppKit 生命周期、导航 delegate、页面代际、页面状态、截图编码、焦点和视图几何 |

## 3. 对外 API 面

`normalize_url(&str) -> Result<String, String>` 规范化地址；`BrowserView::new(WindowHandle)` 借用原生父视图创建浏览器。`navigate` / `back` / `forward` / `reload` / `stop` 执行手动导航；`state()` 返回 `BrowserState { url, title, loading, can_go_back, can_go_forward, error }`；`generation()` 返回当前页面代际（2026-10-07 CU-14，Desktop 多标签回执标注用）。`set_bounds(x,y,width,height)` 使用父视图左上角原点的逻辑像素；`set_visible` 切换显示，`is_focused` 查询网页是否持有原生焦点，`focus_parent` 把键盘交回宿主而不隐藏网页；Drop 释放。只能在 AppKit 主线程使用，不可 Send / Sync。

`read_page(callback)` 返回有界 JSON 页面文本、链接、输入元素和按钮（附 CSS 选择器、视口 rect 与一次性元素句柄），并附页面 `generation`、`dom_revision` 与 `viewport`；completion 回调核对页面代际，派发与回调之间发生导航即明确拒绝（"页面在读取期间导航"），返回的 url/title 以同次脚本在页面读到的值为准（派发前 state 仅作脚本缺失时的回退）。`click(selector, callback)` / `type_text(selector, text, callback)` 要求唯一可见元素；`click_handle(handle, callback)` / `type_handle(handle, text, callback)` 改按 read 签发的句柄定位元素，脚本内先过新鲜度闸（观测 key 存在 + `location.href` 与签发时一致 + `dom_revision` 未变），再跑同一套唯一可见检查。`snapshot(callback)` 经 `takeSnapshotWithConfiguration` 在网页内容进程离屏渲染，返回 JPEG base64、页面与图像尺寸及 `generation`；completion 回调核对代际，截图渲染期间导航 / 内容进程终止时像素不再属于标签声称的页面，明确拒绝（"页面在截图期间导航"）；不激活窗口、不抢焦点，视图隐藏时可用。错误通过 completion 回调返回。固定 DOM 脚本运行于系统 WebKit 的隔离 `WKContentWorld`（`pawork-observe`，求值经 `evaluateJavaScript:inFrame:nil inContentWorld:completionHandler:`——SDK 只声明 inFrame 形态）：观测状态（revision 计数器）不暴露给页面主世界，网页无法枚举或改写它伪造新鲜度；状态仅在 Observer 安装成功后发布——安装或存储失败时脚本显式拒绝、不签发句柄，且不残留 `{revision:0}` 让后续 read 跳过安装检查。不开放任意脚本求值，拒绝密码和文件输入。Host 经 GUI 1.18 请求/回执授权自动操作；本包没有 Host / 协议依赖。

## 4. 核心行为与数据流

2026-09-24：`src/external.rs` 新增 `ExternalBrowser::{Chrome,Edge}` 、`ExternalPageError` 与 `capture_external_page`。用户显式选择后，在后台串行调用 macOS NSAppleScript 读取所选浏览器当前页；不读取 profile、Cookie 或密码字段、不导航。仅 HTTP(S) 无内嵌凭据页面，正文最多 8192 字符，最终 JSON ≤64 KiB，附来源及不可信快照说明。依赖既有 cocoa/objc/serde_json/url；其它平台明确返回不支持。需要系统 Automation 权限及浏览器允许 Apple Events JavaScript，Desktop bundle 声明权限用途。

2026-10-07（CU-15）：新增外部 Chrome / Edge 真实后台控制通道（`src/ext/` + `pawork_browser_bridge` 二进制 + `extension/`）。CDP 已实证不可行（Chrome ≥136 默认 profile 拒绝 remote debugging），改走自有 MV3 扩展 + native messaging：浏览器按 NativeMessagingHosts manifest 拉起桥进程（argv 必须是钉死扩展 origin；扩展 manifest 声明 `nativeMessaging` 权限与 `<all_urls>` host 权限——`captureVisibleTab` 的检查口径只认 `<all_urls>` 或当次用户手势授予的 activeTab，后台截图无手势、activeTab 语义不成立，http/https host 权限不够；`<all_urls>` 虽比 http/https 宽，但桥与扩展的 origin 闸仍只接受 http(s)，不扩大可操作页面范围。静态回归钉住），桥在发现目录发布 unix socket 路径与 32 字节随机 token（0600 原子写），Desktop 客户端逐动作扫描 + hello 验 token + 单启用规则（多浏览器同时启用即拒绝）。固定动作集 `tabs / tab_state / new_tab / navigate / read / click / type / screenshot / close_tab / close_owned`，`executeScript` 页面函数自包含（序列化隔离，引用不到 worker 顶层符号）；read / click / type 的页面函数在触碰 DOM 前先核对 `expectOrigin` 与 `location.origin`，不符即返回 site_changed 哨兵；navigate 的 `bound_origin` 由桥随调用下发（跨 origin 拒绝、重定向逃逸报 site_changed 并解绑），未绑定标签首次 navigate 仅在最终 URL 等于目标时绑定，重定向逃逸或取不到 origin 即报错且不绑定。click 对 password 输入 fail-closed 拒绝（read / type 原有密码防护不变）。screenshot 用 `captureVisibleTab`（要求标签为其窗口 active 且非最小化，如实报错），激活 / 导航监听在首次异步读取状态之前安装：记录目标窗口的全部激活变化（含切回目标标签），监听保护下复核目标后捕获，回传前再重取标签复核 active / url / origin，任何变化丢弃截图并要求重试。句柄 `xt-<pid>-<ns>-<seq>-<tabId>` 绑定桥进程代际，桥重启即全部失效。站点授权与 CU-16 同构：attach 时钉 origin（归一化见 `ext/origin.rs`，接受 IPv4 字面量如 `http://127.0.0.1:8933`，IPv6 仍拒绝），每次动作扩展侧原子复核，site_changed 即解绑。归属与绑定均按任务记账：Host 工具为动作盖章 `task = run_id`，桥强制校验（非空、≤128）并透传给扩展，`owned_tasks`（task → tabId 集合）记 `chrome.storage.session`，Agent 只能关本任务建的标签；站点绑定以 (task, tab) 为键，attached / read / click / type / screenshot / navigate 只认本任务 attach 或首航建立的绑定，site_changed 与标签关闭按标签作废所有任务的绑定。enabled 闸取 `ext_call` 锁后再检查，SW 收到 host 消息执行 handler 前再查 `isEnabled()`。MV3 SW 会被回收：桥每 15s 发 ping + SW 30s alarm 重连保活。扩展由用户在浏览器里确认安装（chrome://extensions 加载已解压 + popup 开关）；Chrome ≥137 品牌版忽略 `--load-extension`，不能靠命令行注入。

创建时空白，不联网。输入域名补 `https://`，localhost / loopback（含端口）补 `http://`；显式 HTTP(S) 保留。浏览历史、URL、标题与加载状态来自 WebKit；错误回调保存真实失败，取消导航不冒充故障。新窗口链接在当前页打开；启用 WebKit `tabFocusesLinks`，Tab 可遍历网页链接和表单控件。隐藏不清空页面或历史，关闭停止加载并释放原生视图、delegate 与网站数据。

2026-10-07（CU-13）：观测语义与 CU-02 目标契约同构。页面代际 `generation` 在每次 provisional 导航与网页内容进程终止时 +1；`dom_revision` 是 read 脚本安装的 `MutationObserver` 计数（key 含随机后缀，导航后 window 清空即天然失效）。元素句柄 `e-<pid>-<启动纳秒>-<进程级序号>` 只存内存，绑定签发时的 selector、页面 URL、revision 与 generation，60 秒过期、最多使用一次、进程内上限 512（FIFO 逐出）；兄弟句柄不因单个元素失效而连坐，DOM revision 变化是失效信号。read 输出仍封顶 64 KiB：注入句柄后先压 `text`，再弹最长元素列表。截图最长边 ≤1280px、≤512KiB（质量阶梯 75/50/30，白底填充透明区域）。视图创建默认 1024×768 视口，保证隐藏观测也有真实布局。

## 5. 契约与不变量

只接受 HTTP(S) 页面导航（内部空页除外），拒绝空输入、带凭据地址、file / data / javascript / 自定义 scheme。网页没有访问宿主业务的桥接。导航策略同时检查地址栏、链接与重定向；不把非法页面交给系统外部应用。布局及遮挡由消费者按实际绘制范围同步，包负责 AppKit 坐标转换和隐藏时归还焦点。

## 6. 依赖关系

`url`、`serde_json` 与 `raw-window-handle`；macOS 使用既有版本的 `cocoa`、`objc`、`block` 和系统 WebKit framework。唯一消费者是 [Desktop](desktop.md)；Desktop bundle 的 `NSAllowsArbitraryLoadsInWebContent` 允许系统网页 HTTP（含本地预览），不改变 Host 网络策略。不影响 `pawork` CLI 的编译依赖闭包。

CU-15 扩展通道追加：`pawork-browser` 自身新增 `[[bin]] pawork_browser_bridge` 与 macOS `libc`（proc_pidpath 父进程识别）；扩展 JS 运行在浏览器自身运行时，不违反纯 Rust 构建链红线。桥进程由浏览器按 manifest 拉起，不属于 Desktop 或 Host 的编译闭包。

## 7. 测试与验证资产

URL 主路径及拒绝边界、Unicode 输出预算与 DOM 回执的定向单元测试；观测覆盖句柄生命周期（一次性 / 代际 / TTL / 容量逐出）与预算收缩、观测脚本断言（MutationObserver、rect、新鲜度闸与转义）、脚本元数据优先与 state 回退（CU-13 复审：同次脚本 url/title 为准）、观测状态安装失败显式拒绝（不降级签发句柄）、状态发布仅在 Observer 安装成功后（红证：不残留 revision 让后续 read 跳过安装检查）。Desktop 测试覆盖入口、布局、任务归属与生命周期，真实导航以本地 HTTP fixture、请求日志和真实窗口联合核对。未闭合验收见 [ROADMAP](../../ROADMAP.md)。

CU-15 资产：ext::{origin,protocol,discovery} 单测（归一化边界含 IPv4 字面量、句柄代际、帧上限与截断前缀报错、发现文件 round trip 与坏文件跳过）与 manifest 权限静态回归（nativeMessaging + <all_urls>）；tests/ext_bridge.rs 集成测试驱动真二进制子进程 + fake 扩展 stdio（Chrome 是唯一 mock 边界），覆盖 token 鉴权、enabled 门、句柄签发与旧句柄拒绝、attach origin 钉定与 mismatch 拒绝、跨源 navigate 拒绝、同源导航保持绑定、未绑定首导航重定向逃逸拒绝、任务作用域归属与绑定隔离（跨任务 close 拒绝、跨任务 read / navigate 借绑定拒绝、缺 task 拒绝）、close_tab 归属闸、close 只报本桥句柄与未知动作拒绝。fake 扩展同时建模按标签一致的页面状态（用户与非用户标签导航 / 重定向均更新当前 URL，重定向后旧 origin attach 拒绝）、read 输入 / 按钮列表（字段与 UTF-16 长度对齐真实扩展）、click / type、password / file 拒绝、site drift 与截图非 active / 最小化 / synthetic fixture / 超限分支；模拟主路径与断线重连测试覆盖完整动作流、用户 / 跨任务标签保留、发现文件权限与旧句柄失效。验收 JSON 仅在 PAWORK_CU15_ACCEPTANCE_EVIDENCE_DIR 非空时写入；默认不生成验收 JSON，测试仍使用系统临时目录。

## 8. 注意事项与已知限制

首版只支持 macOS；其它平台明确返回不支持。不持久化 URL / Cookie / 历史到下次启动；元素句柄同样只在内存存活。本包仍是单页视图；多网页标签由 Desktop 组合多个 `BrowserView` 实现（2026-10-07 CU-14，见 [desktop Spec](desktop.md)），同样不持久化到下次启动。未提供下载、文件选择或任意脚本执行。2026-09-16 聊天控制通过 Host 的 browser 工具经 Policy / 审批授权，由 Desktop 操作 WebView，结果沿用持久化工具事件。页面文本与截图作为未信任数据返回；截图 JPEG 最长边 1280px、≤512KiB，仅回传给声明 `image_input` 的模型（门控在 Host 的 browser 工具，见 [app Spec](app.md)）。

2026-09-25 外部页面读取失败使用 `ExternalPageError`：Chromium 错误 12 映射 JavaScriptDisabled，系统 -1743 映射 AutomationDenied；其它失败保留数字码，不返回原始错误字典或页面内容。Desktop 分别显示手动启用浏览器脚本和系统自动化授权指引。Chromium 只接受真实用户输入切换脚本设置，自动化点击不会生效；参见 [Chromium 实现](https://github.com/chromium/chromium/blob/main/chrome/browser/ui/browser_commands_mac.mm)。
