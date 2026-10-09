# CU-15 · 外部 Chrome / Edge 的真实后台控制

> 状态：实现与自动验证完成；真实桥 + fake extension 模拟人工验收通过；独立 review / 复审 PASS；真实浏览器 / 系统安装 / 真实像素与竞态待环境。前置：[CU-01](cu-01-background-feasibility.md)、[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

通过 Rust 连接或原生消息通道控制授权浏览器标签页，补齐只读上下文以外的能力。

## 当前依据与读取范围

当前 external.rs 经 AppleScript 读取当前页 URL / 标题 / 有界正文，不导航或控制标签。LCU 浏览器接入依赖官方扩展和安装应用组件，不能直接当独立开源引擎。

[browser Spec](../spec/crates/browser.md)、[app Spec](../spec/crates/app.md)、[protocol Spec](../spec/crates/protocol.md)、[官方 Browser](https://learn.chatgpt.com/docs/browser)。

## 实施范围

- 先验证目标浏览器实际提供的授权 CDP 端点；不能假定普通已打开 profile 自动可连接。若必须控制普通用户标签，采用自有浏览器扩展与 Rust native messaging，保持 Core 和构建链无 Node/V8。
- 浏览器选择、标签 ID、网站授权、导航与重定向绑定真实连接代际；只操作明确目标。
- 实现截图、读取、点击、输入、导航及标签生命周期，不用前台激活或全局键鼠完成网页操作。
- 开发模式下受控 CDP 能力与一般网页动作区分，页面脚本不能调用宿主工具或绕过审批。
- 扩展由用户确认安装/启用，不复制官方私有组件；沿用 Host 授权链，必要布局或依赖变化先在 CU-02 中裁决。

## 完成条件与验证

- 真实 Chrome / Edge 中完成后台页面流程，用户在其它应用输入不中断。
- 用户已有标签不被清理，网站范围、连接断开和旧标签句柄正确拒绝。
- 有页面/请求/文件等独立结果与截图；不能把扩展发现或只读快照记为控制成功。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

实现（2026-10-07，写入集 browser / app / desktop 三包 + 四篇 Spec 与 architecture）：pawork-browser 新增 `src/ext/`（origin / protocol / discovery / host / client）与 `pawork_browser_bridge` 二进制、`extension/` 固定功能 MV3 扩展（钉死 ID `hgalnnlhankacinahkjmflinpobpanjh`）。CDP 已实证不可行（本机 Chrome 154 / Edge 154 运行中、9222 无监听、Chrome ≥136 默认 profile 拒绝 remote debugging），按任务书授权改走自有扩展 + native messaging。桥进程由浏览器按 NativeMessagingHosts manifest 拉起（argv origin 钉死校验），在 `<PAWORK_DATA_DIR>/external-browser/` 发布 socket 路径 + 32 字节随机 token（0600 原子写）；Desktop 侧 client 逐动作扫描 + hello 验 token + 单启用规则。句柄 `xt-<pid>-<ns>-<seq>-<tabId>` 绑定桥代际；站点授权镜像 CU-16（attach 钉 origin、每动作扩展侧原子复核 `expect_origin`、跨源 navigate 拒绝、重定向逃逸 site_changed 解绑）；归属标签记 `chrome.storage.session`，Agent 只能关自己建的标签；screenshot 用 `captureVisibleTab`（非 active / 最小化如实报错）；MV3 保活 = 桥 15s ping + SW 30s alarm 重连。pawork-app 新增独立 `external_browser` 工具（干净动作名翻译 `ext_` 前缀，复用 GUI 1.18 BrowserBroker，wire 零改动；URL 闸与截图 Image part 转换提取为共享 helper）；旧 Desktop 对未知 `ext_*` 动作 fail-closed，无需方言探测。apps/desktop 对 `ext_` 前缀旁路内嵌 WebView 可见性 / 忙碌闸，`spawn_blocking` 调 ext client 后仍经 BrowserRespond 回执，不占用 `browser_request` 槽。

实际命令与结果（2026-10-07，仓库统一 kache wrapper）：`bash scripts/test.sh browser app` 全绿（app 288+2+1，含新增 external_browser 4 项：validate 矩阵、schema enum、screenshot 视觉门先于派发、`ext_` 方言翻译 + Image part；browser 19+2，含 ext 单测与 `tests/ext_bridge.rs` 真二进制集成）；`bash scripts/test.sh desktop` 全绿（262 passed）。修复过程要点：macOS accept 的 socket 继承 listener O_NONBLOCK，serve_client 读到 WouldBlock 误当断连关连接——接受后显式恢复阻塞模式（配 read timeout）；帧前缀截断返回错误而非干净 EOF。

真实结果：待执行。验收环境已就绪：桥二进制 + Chrome NativeMessagingHosts manifest（`~/Library/Application Support/Google/Chrome/NativeMessagingHosts/dev.pawork.browser_bridge.json`，path 指向 target/debug/pawork_browser_bridge）、独立 profile Chrome（`/tmp/cu15-profile`，`open -na` 启动；`--load-extension` 被 Chrome ≥137 品牌版忽略，已确认扩展未注入——安装必须走 chrome://extensions「加载已解压的扩展程序」，这本身就是任务书要求的用户确认安装路径）、本地 fixture HTTP（127.0.0.1:8933，请求日志 `/tmp/cu15/http.log`）。阻塞（2026-10-08 更新）：验收机已解锁；扩展安装须用户当面操作确认（chrome://extensions「加载已解压的扩展程序」），确认后按计划执行 tabs→new_tab→navigate→read→type→click→screenshot→close_tab/close 全流程 + 用户标签保留 + 跨源拒绝 + 断线旧句柄拒绝 + 用户侧输入不中断采样 + Edge 连通 smoke，证据落 `/tmp/cu15/`。

sol 审查修复（2026-10-07 第二轮，9 项全部落地）：① 扩展 manifest 声明 `nativeMessaging` 权限，静态回归 `extension_manifest_declares_native_messaging` 钉住；② read / click / type 页面函数在 DOM 访问前核对 `expectOrigin`，不符返回 site_changed 哨兵；③ `ext_navigate` 改走 `call_tab_scoped` 并下发 `bound_origin`（原扩展侧检查从不执行），site_changed / tab_gone 统一解绑；④ 未绑定标签首次 navigate 仅在最终 URL 等于目标时绑定，重定向逃逸或取不到 origin 报错且不绑定（红证 `bridge_refuses_redirect_escape_on_first_navigation`）；⑤ screenshot 捕获期间挂 `onActivated` / `onUpdated` 监听并在回传前重取标签复核 active / url / origin，变化即丢弃并要求重试；⑥ 归属改任务作用域：Host 工具盖章 `task = run_id`，桥强制校验（非空、≤128）并透传，扩展 `owned_tasks`（task → tabId 集合）记 `chrome.storage.session`（红证 `bridge_ownership_is_task_scoped`）；⑦ password 输入点击 fail-closed 拒绝；⑧ origin 闸接受 IPv4 字面量（`http://127.0.0.1:8933` 等），IPv6 仍拒；⑨ `extension_gate` 检查移到 `ext_call` 锁后，SW `onHostMessage` 执行 handler 前再查 `isEnabled()`。门禁：`bash scripts/test.sh browser app` 全绿（app 288+2+1、browser 20+4，含上述红证与 manifest / IPv4 静态回归）；桥二进制已重建（`cargo build --offline -p pawork-browser --bin pawork_browser_bridge`，真实验收用）。真实 Chrome 连接复验仍受锁屏阻塞：静态核对 + 本记录登记，解锁后随真实验收一并补做。

sol 复审修复（2026-10-07 第三轮，3 项全部落地）：① manifest host permissions 改为 `<all_urls>`——`captureVisibleTab` 的权限检查（Chromium permissions_data.cc 口径）只认 `<all_urls>` 或当次用户手势授予的 activeTab，后台截图无手势、http/https host 权限不满足，真实截图会被拒；`<all_urls>` 虽宽于 http/https，桥与扩展的 origin 闸仍只接受 http(s)，不扩大可操作页面范围。静态回归同时钉住 `nativeMessaging` 与 `<all_urls>`。② 桥的站点绑定改按 (task, tab) 存取：attached / read / click / type / screenshot / navigate 都只认本任务 attach 或首航建立的绑定，任务 B 借不到任务 A 的绑定；未绑定 navigate 仍走 `require_owned` 本任务归属检查（fake 扩展同步对齐该语义）；site_changed 与标签关闭按标签作废所有任务的绑定（红证 `bridge_bindings_are_task_scoped`）。③ sw.js 截图的激活 / 导航监听改在首次异步状态读取之前安装，先记录全部激活事件、拿到目标 windowId 后再过滤——目标窗口任何激活变化（含切回目标标签）都丢弃截图要求重试，消除「读到 A active → windows.get 等待期间切 B → 捕获 B → 切回 A」把 B 像素标成 A 的竞态（JS 侧行为，无 Rust 可测面，真实验收覆盖）。门禁：`bash scripts/test.sh browser` 全绿（20+5）；桥二进制已重建。真实验收仍待解锁。

## 模拟人工验收（2026-10-10）

按用户边界不安装真实扩展、不修改 NativeMessagingHosts / TCC / 用户 profile，也不启动真实 Chrome / Edge。验收链为真实 pawork_browser_bridge 二进制 + 真实 pawork_browser::ext::client::execute + fake extension stdio；Desktop ext_ 分流仅做源码核对，未启动 GPUI，因此不冒充完整 Desktop 真窗验收。

既有 [ext_bridge.rs](../../crates/browser/tests/ext_bridge.rs) 的 fake extension 补齐 read 输入/按钮列表、click / type、password / file 拒绝、site drift、screenshot 非 active / 最小化 / synthetic fixture / 超限分支，并让 new_tab 回显请求 URL；页面状态按标签维护，用户与非用户标签的成功导航和重定向都更新当前 URL，navigate 后 tabs / read / screenshot / close_tab 使用同一 URL；重定向后旧 origin attach 被拒绝，继续 a.test 场景前由 fixture 显式恢复并记录，read 输入字段对齐 kind 等真实扩展字段，type 长度按 UTF-16 计数。新增模拟主路径覆盖 tabs → new_tab → navigate → read → type → click → close_tab / close；用户与其它任务标签保留，跨任务 close 拒绝；attach / expect_origin、跨源 navigate、重定向逃逸解绑、用户标签重定向后旧 origin attach 的 mismatch 拒绝与旧句柄拒绝；发现文件 0600 / 目录 0700、token 长度 64、错误 token 拒绝、SIGKILL 后重连拒绝旧句柄。截图回执为 synthetic fixture，不声明真实 DOM 像素或激活 / 导航竞态。

实际验证：首次 cargo test -p pawork-browser --offline --test ext_bridge -- --nocapture 在既有归属测试按旧固定 URL 查找时失败（exit 101，flow / restart 当次已通过）；对齐请求 URL 后同命令通过（7 passed）。证据输出由非空 PAWORK_CU15_ACCEPTANCE_EVIDENCE_DIR 显式启用；带该变量重跑 7 passed 并生成 flow / restart JSON，复审修复同一标签 URL 状态后，带变量与去掉变量均重跑 7 passed；未设置变量时不生成验收 JSON（测试仍使用系统临时目录）。证据在 /tmp/pawork-cu15-external-browser-simulation/（含成功 / 失败日志、conclusion 与脱敏 JSON；token 只记录长度，测试口令不落证据）。

待验边界：真实 Chrome / Edge 安装与启用、NativeMessagingHosts 拉起、真实 DOM / HTTP / 用户输入连续性、真实截图像素与激活 / 导航竞态、Edge smoke。以上需要用户确认安装或真实浏览器环境，不能用 fake extension 结果替代。

独立 review（2026-10-10）：执行者为线程 01a122e9-9fdd-7310-a4c2-c3ff47e4107f（xai/grok-4.7 / xhigh）。R1c 受审快照 /tmp/pawork-cu15-external-browser-r1c.diff，55742 字节，SHA256 dbe1c6c6e86522694e269cc800a7cb72d50ff1f19ba6646253b3155d8ec37eb9；reviewer 复核 R1c 与当时工作区一致。唯一 reviewer 线程 01a122fe-23b2-73d3-a116-e0e2ddba5895（gpt-6.1-sol / max）初审 turn 01a122fe-2760-7b23-a257-000bfdae5068 指出模拟回执 URL 状态矛盾（P2）与输入字段 / UTF-16 长度、副作用文案偏差（P3）；修复后第二轮 turn 01a1230b-fc3c-7581-97f4-5cef3e3b3db8 继续指出用户标签重定向后旧 origin attach 误通过（P2）；执行者补齐所有导航更新当前 URL、旧 origin mismatch 拒绝与显式 fixture 恢复后，最终 turn 01a12311-c8c8-7310-bb93-ed7e1e85d5c0 VERDICT: PASS。reviewer 全程只读，不运行 Cargo。
