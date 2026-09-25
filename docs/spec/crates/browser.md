# pawork-browser

> 系统网页视图库。首版 macOS WebKit，供 Desktop 的任务浏览器使用；无 `pawork-*`、GPUI、Core 或协议依赖。

## 1. 职责与边界

承载手动导航及宿主授权 DOM 操作的原生视图、HTTP(S) 导航、历史、状态与生命周期。纯 Rust 调用系统 WebKit，不引入 Node / Bun / V8 或自有 JS Runtime；网页脚本由系统内容进程执行。每个视图使用隔离的非持久网站数据，不继承 Safari / Chrome profile。没有网页到 Rust 的 IPC、模型调用或自定义本地资源协议；自动操作只接受宿主调用，网页不能主动调用工具。

## 2. 模块与文件地图

| 路径 | 内容 |
| --- | --- |
| `src/lib.rs` | `BrowserState`、地址规范化、公开 facade、非 macOS 不支持分支与定向测试 |
| `src/external.rs` | 显式读取 Chrome/Edge 当前页的有界只读快照 |
| `src/dom.rs` | 固定 DOM 脚本、参数转义、唯一可见元素检查、JSON 输出预算与定向测试 |
| `src/macos.rs` | WebKit / AppKit 生命周期、导航 delegate、页面状态、焦点和视图几何 |

## 3. 对外 API 面

`normalize_url(&str) -> Result<String, String>` 规范化地址；`BrowserView::new(WindowHandle)` 借用原生父视图创建浏览器。`navigate` / `back` / `forward` / `reload` / `stop` 执行手动导航；`state()` 返回 `BrowserState { url, title, loading, can_go_back, can_go_forward, error }`。`set_bounds(x,y,width,height)` 使用父视图左上角原点的逻辑像素；`set_visible` 切换显示，`is_focused` 查询网页是否持有原生焦点，`focus_parent` 把键盘交回宿主而不隐藏网页；Drop 释放。只能在 AppKit 主线程使用，不可 Send / Sync。

`read_page(callback)` 返回有界 JSON 页面文本、链接、输入元素和按钮（附 CSS 选择器）。`click(selector, callback)` / `type_text(selector, text, callback)` 要求唯一可见元素，错误通过 completion 回调返回。固定 DOM 脚本运行于系统 WebKit，不开放任意脚本求值，拒绝密码和文件输入。Host 经 GUI 1.18 请求/回执授权自动操作；本包没有 Host / 协议依赖。

## 4. 核心行为与数据流

2026-09-24：`src/external.rs` 新增 `ExternalBrowser::{Chrome,Edge}` 、`ExternalPageError` 与 `capture_external_page`。用户显式选择后，在后台串行调用 macOS NSAppleScript 读取所选浏览器当前页；不读取 profile、Cookie 或密码字段、不导航。仅 HTTP(S) 无内嵌凭据页面，正文最多 8192 字符，最终 JSON ≤64 KiB，附来源及不可信快照说明。依赖既有 cocoa/objc/serde_json/url；其它平台明确返回不支持。需要系统 Automation 权限及浏览器允许 Apple Events JavaScript，Desktop bundle 声明权限用途。

创建时空白，不联网。输入域名补 `https://`，localhost / loopback（含端口）补 `http://`；显式 HTTP(S) 保留。浏览历史、URL、标题与加载状态来自 WebKit；错误回调保存真实失败，取消导航不冒充故障。新窗口链接在当前页打开；启用 WebKit `tabFocusesLinks`，Tab 可遍历网页链接和表单控件。隐藏不清空页面或历史，关闭停止加载并释放原生视图、delegate 与网站数据。

## 5. 契约与不变量

只接受 HTTP(S) 页面导航（内部空页除外），拒绝空输入、带凭据地址、file / data / javascript / 自定义 scheme。网页没有访问宿主业务的桥接。导航策略同时检查地址栏、链接与重定向；不把非法页面交给系统外部应用。布局及遮挡由消费者按实际绘制范围同步，包负责 AppKit 坐标转换和隐藏时归还焦点。

## 6. 依赖关系

`url`、`serde_json` 与 `raw-window-handle`；macOS 使用既有版本的 `cocoa`、`objc`、`block` 和系统 WebKit framework。唯一消费者是 [Desktop](desktop.md)；Desktop bundle 的 `NSAllowsArbitraryLoadsInWebContent` 允许系统网页 HTTP（含本地预览），不改变 Host 网络策略。不影响 `pawork` CLI 的编译依赖闭包。

## 7. 测试与验证资产

URL 主路径及拒绝边界、Unicode 输出预算与 DOM 回执的定向单元测试；Desktop 测试覆盖入口、布局、任务归属与生命周期，真实导航以本地 HTTP fixture、请求日志和真实窗口联合核对。具体结果见 历史记录（Git `f8df04b2:docs/ROADMAP.md`，原「右侧浏览器首版2026-09-16」节）；无全 workspace 门禁。

## 8. 注意事项与已知限制

首版只支持 macOS；其它平台明确返回不支持。每任务一个浏览器页，不持久化 URL / Cookie / 历史到下次启动。未提供多网页标签、下载、文件选择、任意脚本执行或截图读取。2026-09-16 聊天控制通过 Host 的 browser 工具经 Policy / 审批授权，由 Desktop 操作 WebView，结果沿用持久化工具事件。页面文本作为未信任数据返回。

2026-09-25 外部页面读取失败使用 `ExternalPageError`：Chromium 错误 12 映射 JavaScriptDisabled，系统 -1743 映射 AutomationDenied；其它失败保留数字码，不返回原始错误字典或页面内容。Desktop 分别显示手动启用浏览器脚本和系统自动化授权指引。Chromium 只接受真实用户输入切换脚本设置，自动化点击不会生效；参见 [Chromium 实现](https://github.com/chromium/chromium/blob/main/chrome/browser/ui/browser_commands_mac.mm)。
