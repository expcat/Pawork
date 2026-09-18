# pawork-browser Review

> 系统网页视图库（首版 macOS WebKit）：手动导航、宿主授权的受限 DOM 操作、页面状态与生命周期管理。3 个 .rs 文件共 869 行（lib 172 / dom 272 / macos 425，含 4 个内联单元测试），无 pawork-* / GPUI / Core / 协议依赖；唯一消费者是 apps/desktop 的任务浏览器。

## 1. 职责与边界

承载手动导航及宿主授权 DOM 操作的原生视图、HTTP(S) 导航、历史、状态与生命周期。纯 Rust 调用系统 WebKit，不引入 Node / Bun / V8 或自有 JS Runtime，网页脚本由系统内容进程执行。每个视图使用隔离的非持久网站数据（WKWebsiteDataStore nonPersistent），不继承 Safari / Chrome profile。没有网页到 Rust 的 IPC、模型调用或自定义本地资源协议；固定 DOM 脚本不开放任意脚本求值，自动操作只接受宿主调用（Host 经 GUI 1.18 请求 / 回执授权），网页不能主动调用工具。

## 2. 依赖关系

| 方向 | 包 | 用途 |
|---|---|---|
| 本包依赖 | 无 pawork-* | 独立于 Core / 协议 / GUI framework |
| 被依赖（生产） | apps/desktop | 唯一消费者（desktop 直接依赖白名单含 pawork-browser） |

| 外部 crate | 用途 |
|---|---|
| url | 地址解析与 scheme/host 判定 |
| serde_json | DOM 脚本回执 JSON 编解码与预算裁剪 |
| raw-window-handle 0.6 | 借用原生父视图（WindowHandle → AppKit ns_view） |
| cocoa =0.26 / objc 0.2 / block 0.1（仅 macOS） | AppKit / WebKit 消息发送与 completion block |

无 feature；非 macOS 平台编译 unsupported 桩（BrowserView::new 返回仅支持 macOS 错误）。不影响 pawork CLI 的编译依赖闭包。

## 3. 文件清单

| 相对路径 | 行数 | 职责 |
|---|---:|---|
| src/lib.rs | 172 | BrowserState、normalize_url 地址规范化、navigation_allowed、非 macOS 不支持分支与 2 个定向测试 |
| src/dom.rs | 272 | 固定 DOM 脚本（read_page / click / type_text）、参数转义（js_string）、唯一可见元素检查、64KiB JSON 输出预算（MAX_OUTPUT_BYTES）与 2 个定向测试 |
| src/macos.rs | 425 | WebKit / AppKit 生命周期、导航 delegate（decide_navigation / decide_response / create_webview）、页面状态、焦点与视图几何、Drop 释放 |

## 4. 类型与方法功能列表

| 名称 | 种类 | 签名/功能 |
|---|---|---|
| `normalize_url(&str) -> Result<String, String>` | fn | 去空白；无 scheme 补 https://（裸 host 冒号后仅允许数字端口）；localhost / loopback（含 IPv6）补 http://；显式 HTTP(S) 保留；拒绝空输入、/ 开头、控制字符、非 HTTP(S) scheme、带用户名密码的地址 |
| `is_web_url(&url)` | fn（pub(crate)） | scheme ∈ {http, https} 且有 host 且无内嵌凭据 |
| `navigation_allowed(&str)` | fn（cfg macos/test） | about:blank 或 web URL 才放行（地址栏、链接、重定向共用） |
| `BrowserState` | struct | `{ url, title, loading, can_go_back, can_go_forward, error }` 页面状态快照 |
| `BrowserView::new(WindowHandle)` | fn | 借用原生父视图创建 WKWebView：要求 AppKit 窗口 + 主线程；nonPersistent data store；JavaScriptCanOpenWindowsAutomatically=NO；tabFocusesLinks=YES；初始隐藏 |
| `navigate / back / forward / reload / stop` | fn | 手动导航与历史操作；navigate 先过 normalize_url |
| `state()` | fn | 返回 BrowserState；error 来自 delegate 记录的真实失败（NSURLErrorCancelled -999 不冒充故障） |
| `set_bounds(x, y, w, h)` | fn | 父视图左上角原点逻辑像素；自动做 AppKit 非 flipped 坐标转换；非有限值忽略 |
| `set_visible(bool)` / `is_focused()` / `focus_parent()` | fn | 显示切换（隐藏前若持焦点先交还宿主）；网页是否持原生焦点；键盘交回宿主而不隐藏网页 |
| `read_page(callback)` | fn | 有界 JSON 页面快照：文本（≤24KiB）、≤80 条可见链接 / 输入 / 按钮（附 CSS 选择器）；超预算先裁文本再裁数组 |
| `click(selector, callback)` / `type_text(selector, text, callback)` | fn | 要求唯一可见元素；拒绝密码 / 文件输入、download 属性链接、非 web 链接；type_text 仅 INPUT/TEXTAREA/contentEditable；错误经 completion 回调返回 |
| dom.rs 内部 | fn | `require_selector`（拒绝空 / 控制字符）、`decode_envelope`（ok/error 信封）、`encode_success`、`finalize_page_json`（注入权威 url/title、过滤非 web 链接、Unicode 安全截断到 64KiB） |

## 5. 关键行为与契约

- 只接受 HTTP(S) 页面导航（内部空页 about:blank 除外）：地址栏、链接点击（decide_navigation）、重定向响应（decide_response）、新窗口（create_webview 返回 nil 并就地加载）四条路径共用 navigation_allowed；不把非法页面交给系统外部应用。
- 下载拒绝：shouldPerformDownload 或不可显示 MIME → 记录错误并取消。
- 网页没有访问宿主业务的桥接；页面文本 / DOM 回执作为未信任数据返回。
- 隐藏不清空页面或历史；Drop 停止加载、解除 delegate、移出父视图并释放原生对象与网站数据。
- 只能在 AppKit 主线程使用，不可 Send / Sync（PhantomData<Rc<()>> 钉住）。

## 6. 测试资产

全部内联（无独立 tests/ 目录）：

| 位置 | 验证点 |
|---|---|
| lib.rs tests（2） | 地址规范化主路径（https 补全 / localhost / loopback / IPv6 保留 http）与拒绝边界（javascript / data / file / 自定义 scheme / mailto / 凭据 / 控制 / 空输入）；navigation_allowed 对非法导航与 about:blank 的判定 |
| dom.rs tests（2） | finalize_page_json 的 Unicode 截断到 64KiB 预算与字段保序；decode_envelope 选择器失败 / 成功回执 |

真实导航、布局与生命周期由 apps/desktop 测试覆盖（本地 HTTP fixture + 请求日志 + 真实窗口联合核对）。默认验证命令：`cargo test -p pawork-browser --offline --lib --tests`（macOS）。

## 7. 协作关系

```mermaid
graph LR
  desktop[apps/desktop<br/>任务浏览器面板] --> browser
  browser --> webkit[系统 WebKit<br/>内容进程执行网页脚本]
  host[CLI Host<br/>browser 工具经 Policy / 审批] -.GUI 1.18 授权 / 回执.-> desktop
  browser --> deps[url / serde_json / raw-window-handle<br/>+ macOS cocoa / objc / block]
```

