# ENV-05 · Chrome / Edge 只读上下文环境复验

> 状态：正式只读函数与错误分类已在源码核实；临时 example 调用四次均为 Apple Event -1712，未取得正文，Pawork.app 产品链未测。阻塞：需真实 macOS 上用户把 Automation 授予 Pawork.app 并允许浏览器执行 Apple 事件中的 JavaScript。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#环境待验)

## 目标

用户手动授权后复验 Chrome / Edge 当前页正文读取。本项是只读上下文，不是 CU-15 外部浏览器控制（其真实安装验收见 [ENV-08](env-08-macos-real-system.md)）。

## 当前依据与读取范围

[browser Spec](../spec/crates/browser.md)、[Desktop Spec](../spec/desktop.md)。

## 验收范围与完成条件

- 真实 Chrome / Edge 正文读取成功，来源和不可信快照说明正确。
- JavaScriptDisabled / AutomationDenied 等真实拒绝仍准确显示，不自动代用户批准系统权限。

## 已核实（2026-10-12）

- 临时 example 直接调用正式 `pawork_browser::capture_external_page`：Chrome 三次、Edge 一次均为 Other / Apple Event -1712（超时，原因未辨明），没有取得 URL、标题或正文。发起进程是 adhoc 签名的临时 example，不是 Pawork.app，未测定 TCC responsible application。
- 现行源码锚点：`crates/browser/src/external.rs` 用 10 秒 AppleScript 读最前标签、正文截到 8192 并标 `truncated`；错误 12 → `JavaScriptDisabled`，-1743 → `AutomationDenied`，其余（含 -1712）→ Other。Desktop 附件菜单调用同一函数并按三类错误显示不同文案。

## 剩余待验

1. 用已安装的 Pawork.app 附件菜单（不是临时 example）对运行中的 Chrome 与 Edge 最前 http(s) 页读取正文。前置：Automation 权限授予 Pawork，浏览器已允许 Apple 事件中的 JavaScript，同意对话框须在 10 秒超时前完成。成功结果须含 url、title、text 和 “Untrusted read-only page snapshot selected by the user”。
2. 短页成功后再读超过 8192 字符的页，确认 `truncated=true`。
3. 关闭 JavaScript-from-Apple-Events 时显示错误 12 的开发者菜单指引；拒绝自动化时显示 -1743 的系统设置指引。若 -1712 在 Pawork.app 上重现，保持 Other / 超时，不改记成上述两类。

## 阻塞（2026-10-13）

三项都需要真实 macOS、已安装的 Pawork.app、运行中的 Chrome / Edge 与用户完成系统 Automation 授权手势；AppleScript 读取路径只在 macOS 编译。2026-10-13 的工作环境是 Linux 盒（无 `osascript`、无 Chrome / Edge 桌面会话），不能推进，状态沿用 2026-10-12 证据。
